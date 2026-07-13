import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  checkAppUpdate,
  openExternalUrl,
  type AppUpdateInfo,
} from '../pty-client';

const UPDATE_INITIAL_DELAY_MS = 5 * 1000;
const UPDATE_CHECK_INTERVAL_MS = 30 * 60 * 1000;
const DISMISSED_UPDATE_VERSION_KEY = 'kota-v2.update.dismissed-version';
const KOTA_HOME_URL = 'https://kota.place';

function readDismissedVersion(): string | null {
  try {
    return window.localStorage.getItem(DISMISSED_UPDATE_VERSION_KEY);
  } catch {
    return null;
  }
}

function writeDismissedVersion(version: string) {
  try {
    window.localStorage.setItem(DISMISSED_UPDATE_VERSION_KEY, version);
  } catch {
    // localStorage is best-effort UI state; failed persistence should not
    // block the user from dismissing the current in-memory banner.
  }
}

export interface KotaUpdateCheckState {
  updateInfo: AppUpdateInfo | null;
  updateAvailable: boolean;
  latestVersion: string | null;
  openUpdateHome: () => Promise<void>;
  dismissUpdate: () => void;
}

export function useKotaUpdateCheck(): KotaUpdateCheckState {
  const [updateInfo, setUpdateInfo] = useState<AppUpdateInfo | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(() => {
    if (typeof window === 'undefined') return null;
    return readDismissedVersion();
  });

  const check = useCallback(async () => {
    try {
      const info = await checkAppUpdate();
      setUpdateInfo(info.hasUpdate ? info : null);
    } catch {
      // Update checks are non-critical. Offline, blocked, or malformed
      // manifests should leave the app quiet.
    }
  }, []);

  useEffect(() => {
    const initialTimer = window.setTimeout(check, UPDATE_INITIAL_DELAY_MS);
    const interval = window.setInterval(check, UPDATE_CHECK_INTERVAL_MS);
    return () => {
      window.clearTimeout(initialTimer);
      window.clearInterval(interval);
    };
  }, [check]);

  const updateAvailable = useMemo(() => (
    !!updateInfo?.hasUpdate
      && !!updateInfo.latestVersion
      && updateInfo.latestVersion !== dismissedVersion
  ), [dismissedVersion, updateInfo]);

  const openUpdateHome = useCallback(async () => {
    await openExternalUrl(updateInfo?.homeUrl || KOTA_HOME_URL);
  }, [updateInfo?.homeUrl]);

  const dismissUpdate = useCallback(() => {
    const version = updateInfo?.latestVersion;
    if (!version) return;
    writeDismissedVersion(version);
    setDismissedVersion(version);
  }, [updateInfo?.latestVersion]);

  return {
    updateInfo,
    updateAvailable,
    latestVersion: updateInfo?.latestVersion ?? null,
    openUpdateHome,
    dismissUpdate,
  };
}
