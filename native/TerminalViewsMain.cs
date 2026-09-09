// Precompiled entry point; production paths never load PowerShell/Add-Type.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

public static class TerminalViewsMain {
    [STAThread]
    public static int Main(string[] args) {
        try {
            Console.OutputEncoding = new UTF8Encoding(false);
            var values = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            for (int i = 0; i < args.Length; i += 2) {
                if (i + 1 >= args.Length) throw new ArgumentException("Expected an option value.");
                values.Add(args[i], args[i + 1]);
            }
            Func<string,string> get = key => values.ContainsKey(key) ? values[key] : "";
            var mode = get("-Mode");
            if (mode == "List") { Console.WriteLine(TerminalViews.Snapshot()); return 0; }
            if (mode == "State") { Console.WriteLine(TerminalViews.State()); return 0; }
            if (mode != "Track") throw new ArgumentException("Supported modes are Track, List and State.");
            TerminalViews.Track(get("-RecordPath"), get("-InitialTitle"), get("-RuntimeId"),
                Int32.Parse(get("-OwnerPid"), CultureInfo.InvariantCulture), get("-MachineId"), get("-ContextPath"));
            return 0;
        } catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
    }
}
