export const APP_NAME = "Axiom";
export const APP_ID = "stream.axiom.desktop";
export const LINUX_DESKTOP_FILE = `${APP_ID}.desktop`;

export function setLinuxDesktopIdentity(env, platform) {
  if (platform !== "linux") return;
  // IDE terminals inherit their host's desktop identity. Never let KDE/GNOME
  // associate Axiom with the IDE that launched it.
  env.CHROME_DESKTOP = LINUX_DESKTOP_FILE;
  delete env.BAMF_DESKTOP_FILE_HINT;
  delete env.GIO_LAUNCHED_DESKTOP_FILE;
  delete env.GIO_LAUNCHED_DESKTOP_FILE_PID;
}
