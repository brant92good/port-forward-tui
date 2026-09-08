// Compiled once; existing-view switches do not need a PowerShell runtime.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Automation;

public static class FocusHelper {
    static readonly double Started = Now();
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr window, int command);
    static readonly JavaScriptSerializer Json = new JavaScriptSerializer();
    static readonly Condition TabCondition = new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.TabItem);
    static string TracePath;
    static readonly List<object> Trace = new List<object>();
    static double Now() { return Stopwatch.GetTimestamp() / (double)Stopwatch.Frequency; }
    static void Mark(string name) {
        if (TracePath != null) Trace.Add(new { name = name, at = Now() });
    }

    public class ViewRecord {
        public int pid;
        public long started;
        public string runtime_id;
        public long last_focus;
    }

    static bool Alive(ViewRecord record) {
        try {
            using (var process = Process.GetProcessById(record.pid))
                return !process.HasExited && process.StartTime.ToUniversalTime().Ticks == record.started;
        } catch (ArgumentException) { return false; }
          catch (System.ComponentModel.Win32Exception) { return false; }
          catch (InvalidOperationException) { return false; }
    }

    class Tab {
        public string title;
        public string runtime_id;
        public long window;
        public AutomationElement element;
    }

    static List<Tab> Tabs(bool identities) {
        var tabs = new List<Tab>();
        var windows = AutomationElement.RootElement.FindAll(TreeScope.Children,
            new PropertyCondition(AutomationElement.ClassNameProperty, "CASCADIA_HOSTING_WINDOW_CLASS"));
        foreach (AutomationElement window in windows) {
            try {
                foreach (AutomationElement element in window.FindAll(TreeScope.Descendants, TabCondition)) {
                    tabs.Add(new Tab { title = element.Current.Name,
                        runtime_id = identities ? String.Join(".", element.GetRuntimeId()) : null,
                        window = window.Current.NativeWindowHandle, element = element });
                }
            } catch (ElementNotAvailableException) { }
        }
        return tabs;
    }

    static bool Allowed(long origin, long target) {
        long foreground = GetForegroundWindow().ToInt64();
        return foreground == origin || foreground == target;
    }

    static bool WaitForClosedTab(long origin, long target, string title) {
        if (String.IsNullOrEmpty(title)) return true;
        var deadline = DateTime.UtcNow.AddSeconds(2);
        var condition = new AndCondition(TabCondition,
            new PropertyCondition(AutomationElement.NameProperty, title));
        while (DateTime.UtcNow < deadline) {
            if (!Allowed(origin, target)) return false;
            try {
                var window = AutomationElement.FromHandle(new IntPtr(origin));
                if (window.FindFirst(TreeScope.Descendants, condition) == null) return true;
            } catch (ElementNotAvailableException) { return true; }
              catch (ArgumentException) { return true; }
            Thread.Sleep(10);
        }
        return false;
    }

    static bool Activate(long target, long origin, string title, string runtimeId = null) {
        var window = AutomationElement.FromHandle(new IntPtr(target));
        AutomationElement element = null;
        if (runtimeId == null) {
            element = window.FindFirst(TreeScope.Descendants, new AndCondition(TabCondition,
                new PropertyCondition(AutomationElement.NameProperty, title)));
        } else {
            foreach (AutomationElement tab in window.FindAll(TreeScope.Descendants, TabCondition)) {
                if (String.Join(".", tab.GetRuntimeId()) == runtimeId) { element = tab; break; }
            }
        }
        if (element == null) return false;
        Mark("target_resolved");
        var selection = (SelectionItemPattern)element.GetCurrentPattern(SelectionItemPattern.Pattern);
        if (!Allowed(origin, target)) return false;
        var handle = new IntPtr(target);
        if (IsIconic(handle)) ShowWindow(handle, 9);
        selection.Select();
        Mark("tab_selected");
        if (!Allowed(origin, target)) return false;
        if (GetForegroundWindow() != handle && !SetForegroundWindow(handle)) return false;
        Mark("window_focused");
        var condition = new AndCondition(
            new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Text),
            new PropertyCondition(AutomationElement.IsKeyboardFocusableProperty, true),
            new PropertyCondition(AutomationElement.IsOffscreenProperty, false),
            new PropertyCondition(AutomationElement.IsTextPatternAvailableProperty, true));
        var deadline = DateTime.UtcNow.AddSeconds(2);
        while (DateTime.UtcNow < deadline) {
            if (GetForegroundWindow() != handle || !selection.Current.IsSelected) return false;
            try {
                var content = window.FindFirst(TreeScope.Descendants, condition);
                if (content != null) {
                    if (GetForegroundWindow() != handle || !selection.Current.IsSelected) return false;
                    content.SetFocus();
                    var focused = AutomationElement.FocusedElement;
                    if (GetForegroundWindow() == handle && selection.Current.IsSelected && focused != null
                        && focused.GetRuntimeId().SequenceEqual(content.GetRuntimeId())) {
                        Mark("content_focused");
                        return true;
                    }
                }
            } catch (ElementNotAvailableException) { }
            Thread.Sleep(10);
        }
        return false;
    }

    static bool WaitForParent(string pid) {
        try {
            using (var parent = Process.GetProcessById(Int32.Parse(pid)))
                return parent.WaitForExit(5000);
        } catch (ArgumentException) { return true; }
    }

    [STAThread]
    public static int Main(string[] arguments) {
        try {
            var options = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            for (int i = 0; i < arguments.Length; i++) {
                string key = arguments[i];
                options[key] = key == "-ProbeOnly" ? "true" : arguments[++i];
            }
            string value;
            if (options.TryGetValue("-TracePath", out value)) {
                TracePath = value;
                Trace.Add(new { name = "native_entry", at = Started });
            }
            var titles = options.TryGetValue("-TitlesBase64", out value)
                ? Json.Deserialize<string[]>(Encoding.UTF8.GetString(Convert.FromBase64String(value))) : new string[0];
            var records = options.TryGetValue("-RecordsBase64", out value)
                ? Json.Deserialize<ViewRecord[]>(Encoding.UTF8.GetString(Convert.FromBase64String(value))) : null;
            Mark("options_decoded");
            if (options.ContainsKey("-ProbeOnly") || options.ContainsKey("-ReadyEvent")) {
                var tabs = Tabs(records != null);
                Mark("tabs_enumerated");
                long origin = 0;
                if (options.TryGetValue("-OriginTitle", out value)) {
                    var launcher = tabs.FirstOrDefault(t => t.title == value);
                    if (launcher == null) return 1;
                    origin = launcher.window;
                }
                bool local = options.TryGetValue("-Scope", out value) && value == "window";
                if (local && origin == 0) return 1;
                var candidates = new List<Tab>();
                if (records != null) {
                    foreach (var record in records.OrderByDescending(r => r.last_focus)) {
                        if (!Alive(record)) continue;
                        var match = tabs.FirstOrDefault(t => t.runtime_id == record.runtime_id && (!local || t.window == origin));
                        if (match != null) { candidates.Add(match); break; }
                    }
                } else {
                    foreach (string title in titles) {
                        var match = tabs.FirstOrDefault(t => t.title == title && (!local || t.window == origin));
                        if (match != null) { candidates.Add(match); break; }
                    }
                }
                foreach (var target in candidates) {
                    Mark("target_chosen");
                    if (options.TryGetValue("-ReadyEvent", out value)) {
                        if (origin == 0) return 1;
                        // Let the Python launcher exit, then finish in this same
                        // process instead of starting a second focus helper.
                        using (var ready = EventWaitHandle.OpenExisting(value)) ready.Set();
                        if (!WaitForParent(options["-AfterPid"])) return 1;
                        Mark("launcher_exited");
                        if (!WaitForClosedTab(origin, target.window, options["-OriginTitle"])) return 1;
                        Mark("launcher_tab_closed");
                        return Activate(target.window, origin, target.title, target.runtime_id) ? 0 : 1;
                    }
                    Console.OutputEncoding = new UTF8Encoding(false);
                    Console.WriteLine(Json.Serialize(new { title = target.title, runtime_id = target.runtime_id, window = target.window, origin = origin }));
                    return 0;
                }
                return 1;
            }
            long windowHandle = Int64.Parse(options["-WindowHandle"]);
            long invokeWindow = Int64.Parse(options["-InvokeWindow"]);
            if (invokeWindow == 0 || titles.Length != 1) return 1;
            if (options.TryGetValue("-AfterPid", out value)) {
                if (!WaitForParent(value)) return 1;
            }
            options.TryGetValue("-ClosedTitle", out value);
            if (!WaitForClosedTab(invokeWindow, windowHandle, value)) return 1;
            return Activate(windowHandle, invokeWindow, titles[0]) ? 0 : 1;
        } catch {
            return 2;
        } finally {
            if (TracePath != null) {
                try { File.WriteAllText(TracePath, Json.Serialize(Trace), new UTF8Encoding(false)); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
        }
    }
}
