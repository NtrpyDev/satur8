// Satur8 focus forwarder.
//
// KWin does not expose the active window to third-party processes, so this tiny
// script (running inside KWin, which does see it) forwards each window
// activation to the satur8 daemon over D-Bus. It changes nothing about the
// window itself - it only reports "this class is now focused" so the daemon can
// apply or restore the matching per-game profile.
//
// Zero polling: we react only to KWin's own windowActivated signal.

// The pid goes over as a string: callDBus offers no control over the numeric
// type it marshals, and a signature mismatch fails silently.
function report(cls, cap, pid) {
    callDBus("org.satur8.Daemon", "/org/satur8/Daemon",
             "org.satur8.Daemon", "WindowActivatedPid", cls, cap, pid);
}

function reportActive(window) {
    if (!window) {
        // Focus left to the desktop/null - report an empty class so the daemon
        // restores the default.
        report("", "", "0");
        return;
    }
    var cls = window.resourceClass ? "" + window.resourceClass : "";
    var cap = window.caption ? "" + window.caption : "";
    // The pid is what identifies a Proton game. Every non-Steam title launched
    // through umu carries the same window class, so the class alone cannot tell
    // one from another; the daemon resolves this pid to the real executable.
    var pid = window.pid ? "" + window.pid : "0";
    report(cls, cap, pid);
}

// KWin 6 scripting API: workspace.windowActivated(window).
if (workspace.windowActivated) {
    workspace.windowActivated.connect(reportActive);
} else if (workspace.clientActivated) {
    // Fallback for older API naming.
    workspace.clientActivated.connect(reportActive);
}

// Report whatever is focused right now, so applying state doesn't wait for the
// next focus change.
reportActive(workspace.activeWindow || (workspace.activeClient || null));
