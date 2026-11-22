// SPDX-License-Identifier: GPL-3.0-or-later
// The Satur8 panel in the Quick Access Menu: whether Satur8 is installed,
// one button to install or update it, and one to open it. The work is done by
// main.py; this file only shows it and talks to Steam's own library.

import {
  ButtonItem,
  ConfirmModal,
  DialogButton,
  Field,
  Navigation,
  PanelSection,
  PanelSectionRow,
  ProgressBarWithInfo,
  showModal,
  staticClasses,
} from "@decky/ui";
import { callable, definePlugin } from "@decky/api";
import { useCallback, useEffect, useRef, useState } from "react";

interface Status {
  installed: string | null;
  latest: string | null;
  summary: string;
  button: string | null;
  reinstall: string | null;
  reinstall_question: string;
  busy: boolean;
  step: number;
  steps: string[];
  result: string;
  error: string;
  log: string;
  app_name: string;
  app_path: string;
  app_dir: string;
}

const getStatus = callable<[], Status>("status");
const checkLatest = callable<[], string | null>("check_latest");
const startInstall = callable<[inSteamLibrary: boolean], void>("install");

// Steam's app type for a non-Steam shortcut (EAppType.Shortcut).
const SHORTCUT = 1073741824;

// Satur8's entry in Steam's own list of the library, as Steam has it right
// now. Steam writes its shortcuts file later, so this list is the one to ask.
function libraryEntry(name: string) {
  const apps = window.appStore?.allApps ?? [];
  return apps.find((app) => app.app_type === SHORTCUT && app.display_name === name) ?? null;
}

function Content() {
  const [status, setStatus] = useState<Status | null>(null);
  const [inLibrary, setInLibrary] = useState(false);
  const [note, setNote] = useState("");
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const next = await getStatus();
    if (!alive.current) return next;
    setStatus(next);
    setInLibrary(libraryEntry(next.app_name) !== null);
    return next;
  }, []);

  // On opening the panel: what is installed, then what the newest release is.
  useEffect(() => {
    alive.current = true;
    (async () => {
      await refresh();
      await checkLatest();
      await refresh();
    })().catch((error) => setNote(String(error)));
    return () => {
      alive.current = false;
    };
  }, [refresh]);

  // While an install runs, follow its steps.
  const busy = status?.busy ?? false;
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => {
      refresh().catch(() => undefined);
    }, 1000);
    return () => clearInterval(timer);
  }, [busy, refresh]);

  // Steam lists a new entry a moment after it is asked to, so keep looking
  // while Satur8 is installed and not listed.
  const waitingForEntry = status !== null && status.installed !== null && !inLibrary;
  const appName = status?.app_name ?? "";
  useEffect(() => {
    if (!waitingForEntry) return;
    const timer = setInterval(() => {
      if (libraryEntry(appName) !== null) setInLibrary(true);
    }, 2000);
    return () => clearInterval(timer);
  }, [waitingForEntry, appName]);

  if (status === null) {
    return (
      <PanelSection>
        <PanelSectionRow>
          <Field label="Looking for Satur8" focusable />
        </PanelSectionRow>
      </PanelSection>
    );
  }

  const install = async () => {
    setNote("");
    await startInstall(libraryEntry(status.app_name) !== null);
    await refresh();
  };

  // The repair path: the same checked download and installer, asked for
  // once more before it starts.
  const askReinstall = () => {
    showModal(
      <ConfirmModal
        strTitle="Reinstall Satur8"
        strDescription={status.reinstall_question}
        strOKButtonText="Reinstall"
        strCancelButtonText="Cancel"
        onOK={() => {
          install().catch((error) => setNote(String(error)));
        }}
      />,
    );
  };

  const open = () => {
    const entry = libraryEntry(status.app_name);
    if (entry === null) {
      setInLibrary(false);
      return;
    }
    // The call other Decky plugins start a library entry with.
    SteamClient.Apps.RunGame(entry.gameid, "", -1, 100);
    Navigation.CloseSideMenus();
  };

  const addToLibrary = async () => {
    if (libraryEntry(status.app_name) === null) {
      const appId = await SteamClient.Apps.AddShortcut(
        status.app_name,
        status.app_path,
        status.app_dir,
        "",
      );
      SteamClient.Apps.SetShortcutName(appId, status.app_name);
    }
  };

  const stepName = status.steps[status.step - 1] ?? "Starting";
  const message = status.error || status.result || note;

  return (
    <PanelSection>
      <PanelSectionRow>
        <Field label={status.summary} description={message || undefined} focusable />
      </PanelSectionRow>

      {status.busy && (
        <PanelSectionRow>
          <ProgressBarWithInfo
            nProgress={(Math.max(status.step, 0) / status.steps.length) * 100}
            sOperationText={`${stepName} (${Math.max(status.step, 1)} of ${status.steps.length})`}
          />
        </PanelSectionRow>
      )}

      {!status.busy && status.button !== null && (
        <PanelSectionRow>
          <ButtonItem layout="below" onClick={install}>
            {status.button}
          </ButtonItem>
        </PanelSectionRow>
      )}

      {!status.busy && status.installed !== null && inLibrary && (
        <PanelSectionRow>
          <ButtonItem layout="below" onClick={open}>
            Open Satur8
          </ButtonItem>
        </PanelSectionRow>
      )}

      {!status.busy && status.reinstall !== null && (
        <PanelSectionRow>
          <DialogButton
            style={{ minWidth: 0, padding: "6px 10px", fontSize: "12px" }}
            onClick={askReinstall}
          >
            {status.reinstall}
          </DialogButton>
        </PanelSectionRow>
      )}

      {!status.busy && status.installed !== null && !inLibrary && (
        <PanelSectionRow>
          <ButtonItem
            layout="below"
            description="Steam does not list Satur8 yet. Game Mode opens it from the library."
            onClick={addToLibrary}
          >
            Add Satur8 to the Steam library
          </ButtonItem>
        </PanelSectionRow>
      )}
    </PanelSection>
  );
}

// Satur8's mark for the plugin list: a circle, colorless on one side and
// filled on the other.
function Mark() {
  return (
    <svg viewBox="0 0 24 24" width="1em" height="1em" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
    </svg>
  );
}

export default definePlugin(() => ({
  name: "Satur8",
  titleView: <div className={staticClasses.Title}>Satur8</div>,
  content: <Content />,
  icon: <Mark />,
}));
