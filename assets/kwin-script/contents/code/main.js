// Vibrance focus forwarder.
//
// KWin does not expose the active window to third-party processes, so this tiny
// script (running inside KWin, which does see it) forwards each window
// activation to the vibrance daemon over D-Bus. It changes nothing about the
// window itself - it only reports "this class is now focused" so the daemon can
// apply or restore the matching per-game profile.
//
// Zero polling: we react only to KWin's own windowActivated signal.

function reportActive(window) {
    if (!window) {
        // Focus left to the desktop/null - report an empty class so the daemon
        // restores the default.
        callDBus("org.vibrance.Daemon", "/org/vibrance/Daemon",
                 "org.vibrance.Daemon", "WindowActivated", "", "");
        return;
    }
    var cls = window.resourceClass ? "" + window.resourceClass : "";
    var cap = window.caption ? "" + window.caption : "";
    callDBus("org.vibrance.Daemon", "/org/vibrance/Daemon",
             "org.vibrance.Daemon", "WindowActivated", cls, cap);
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
