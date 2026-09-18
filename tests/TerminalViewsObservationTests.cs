using System;

// Exercises the production predicate without querying UI Automation, observing
// the desktop, selecting a tab, or opening a window.
public static class TerminalViewsObservationTests {
    static int checks;

    static void Check(bool condition, string description) {
        if (!condition) throw new Exception(description);
        checks++;
        Console.WriteLine("PASS: " + description);
    }

    public static int Main() {
        const long tracked = 101;
        const long other = 202;
        try {
            Check(!TerminalViews.IsTrackedTabActive(tracked, () => other,
                () => { throw new Exception("Background observation queried selection."); }),
                "another foreground window never invokes the selection provider");
            Check(!TerminalViews.IsTrackedTabActive(tracked, () => 0,
                () => { throw new Exception("Missing foreground queried selection."); }),
                "no foreground window never invokes the selection provider");

            int foregroundReads = 0;
            int selectionReads = 0;
            Check(TerminalViews.IsTrackedTabActive(tracked,
                () => { foregroundReads++; return tracked; },
                () => { selectionReads++; return true; })
                && foregroundReads == 2 && selectionReads == 1,
                "selected foreground tab checks foreground before and after one selection read");
            Check(!TerminalViews.IsTrackedTabActive(tracked, () => tracked, () => false),
                "an unselected tab in the foreground window remains inactive");

            long foreground = tracked;
            Check(!TerminalViews.IsTrackedTabActive(tracked, () => foreground,
                () => { foreground = other; return true; }),
                "switching to another window during selection access rejects the observation");

            int reads = 0;
            Func<bool> selected = () => { reads++; return true; };
            foreground = tracked;
            bool first = TerminalViews.IsTrackedTabActive(tracked, () => foreground, selected);
            foreground = other;
            bool away = TerminalViews.IsTrackedTabActive(tracked, () => foreground, selected);
            foreground = tracked;
            bool returned = TerminalViews.IsTrackedTabActive(tracked, () => foreground, selected);
            Check(first && !away && returned && reads == 2,
                "foreground re-entry is observed without reading selection while away");

            Console.WriteLine("PASS: " + checks + " production-predicate checks; no desktop/UIA actions.");
            return 0;
        } catch (Exception error) {
            Console.Error.WriteLine("FAIL: " + error.Message);
            return 1;
        }
    }
}
