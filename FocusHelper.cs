// Compiled once; existing-view switches do not need a PowerShell runtime.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Automation;

public static class FocusHelper {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr window, int command);
    static readonly JavaScriptSerializer Json = new JavaScriptSerializer();
    static readonly Condition TabCondition = new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.TabItem);

    class Tab {
        public string title;
        public long window;
        public AutomationElement element;
    }

    static List<Tab> Tabs() {
        var tabs = new List<Tab>();
        var windows = AutomationElement.RootElement.FindAll(TreeScope.Children,
            new PropertyCondition(AutomationElement.ClassNameProperty, "CASCADIA_HOSTING_WINDOW_CLASS"));
        foreach (AutomationElement window in windows) {
            try {
                foreach (AutomationElement element in window.FindAll(TreeScope.Descendants, TabCondition)) {
                    tabs.Add(new Tab { title = element.Current.Name,
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

    static bool Activate(long target, long origin, string title) {
        var window = AutomationElement.FromHandle(new IntPtr(target));
        var element = window.FindFirst(TreeScope.Descendants, new AndCondition(TabCondition,
            new PropertyCondition(AutomationElement.NameProperty, title)));
        if (element == null) return false;
        var selection = (SelectionItemPattern)element.GetCurrentPattern(SelectionItemPattern.Pattern);
        if (!Allowed(origin, target)) return false;
        var handle = new IntPtr(target);
        if (IsIconic(handle)) ShowWindow(handle, 9);
        selection.Select();
        if (!Allowed(origin, target)) return false;
        if (GetForegroundWindow() != handle && !SetForegroundWindow(handle)) return false;
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
                        && focused.GetRuntimeId().SequenceEqual(content.GetRuntimeId())) return true;
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
            var titles = Json.Deserialize<string[]>(Encoding.UTF8.GetString(Convert.FromBase64String(options["-TitlesBase64"])));
            if (options.ContainsKey("-ProbeOnly") || options.ContainsKey("-ReadyEvent")) {
                var tabs = Tabs();
                long origin = 0;
                if (options.TryGetValue("-OriginTitle", out value)) {
                    var launcher = tabs.FirstOrDefault(t => t.title == value);
                    if (launcher == null) return 1;
                    origin = launcher.window;
                }
                bool local = options.TryGetValue("-Scope", out value) && value == "window";
                if (local && origin == 0) return 1;
                foreach (string title in titles) {
                    var target = tabs.FirstOrDefault(t => t.title == title && (!local || t.window == origin));
                    if (target == null) continue;
                    if (options.TryGetValue("-ReadyEvent", out value)) {
                        if (origin == 0) return 1;
                        // Let the Python launcher exit, then finish in this same
                        // process instead of starting a second focus helper.
                        using (var ready = EventWaitHandle.OpenExisting(value)) ready.Set();
                        if (!WaitForParent(options["-AfterPid"])) return 1;
                        if (!WaitForClosedTab(origin, target.window, options["-OriginTitle"])) return 1;
                        return Activate(target.window, origin, target.title) ? 0 : 1;
                    }
                    Console.OutputEncoding = new UTF8Encoding(false);
                    Console.WriteLine(Json.Serialize(new { title = title, window = target.window, origin = origin }));
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
        }
    }
}
