// Satur8 focus forwarder.
//
// KWin does not expose the active window to third-party processes, so this tiny
// script (running inside KWin, which does see it) forwards each window
// activation to the satur8 daemon over D-Bus. It changes nothing about the
// window itself - it only reports "this class is now focused" so the daemon can
// apply or restore the matching per-game profile.
//
// Zero polling: we react only to KWin's own windowActivated signal, and to the
// focused window's outputChanged signal.

var SERVICE = "org.satur8.Daemon";
var PATH = "/org/satur8/Daemon";
var IFACE = "org.satur8.Daemon";

// Whether the daemon has answered WindowActivatedOn. A daemon from before
// per-output support has no such method and the call fails without telling
// this script. Until the daemon answers, each report also goes through
// WindowActivatedPid first: an older daemon follows focus with that, and a
// newer one handles both in order and ends on the report that names the
// output.
var daemonTakesOutput = false;

// The pid goes over as a string: callDBus offers no control over the numeric
// type it marshals, and a signature mismatch fails silently.
function report(cls, cap, pid, output) {
    if (!daemonTakesOutput) {
        callDBus(SERVICE, PATH, IFACE, "WindowActivatedPid", cls, cap, pid);
    }
    callDBus(SERVICE, PATH, IFACE, "WindowActivatedOn", cls, cap, pid, output,
             function (accepted) {
                 if (accepted === true) {
                     daemonTakesOutput = true;
                 }
             });
}

// The output a window is on, by the name `satur8 outputs` lists.
function outputName(window) {
    return window && window.output && window.output.name ? "" + window.output.name : "";
}

function reportActive(window) {
    watch(window);
    if (!window) {
        // Focus left to the desktop/null - report an empty class so the daemon
        // restores the default.
        report("", "", "0", "");
        return;
    }
    var cls = window.resourceClass ? "" + window.resourceClass : "";
    var cap = window.caption ? "" + window.caption : "";
    // The pid is what identifies a Proton game. Every non-Steam title launched
    // through umu carries the same window class, so the class alone cannot tell
    // one from another; the daemon resolves this pid to the real executable.
    var pid = window.pid ? "" + window.pid : "0";
    report(cls, cap, pid, outputName(window));
}

// The focused window, followed so a move to another output is reported too.
var watched = null;

function onOutputChanged() {
    if (watched) {
        reportActive(watched);
    }
}

function watch(window) {
    if (watched === window) {
        return;
    }
    if (watched) {
        try {
            watched.outputChanged.disconnect(onOutputChanged);
        } catch (error) {
            // The window was closed and its signals are gone already.
        }
    }
    watched = window || null;
    if (watched && watched.outputChanged) {
        watched.outputChanged.connect(onOutputChanged);
    }
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
