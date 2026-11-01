import { isAbsolute, join } from "node:path";
import { resolveServiceOrigins } from "./service-origins";

/** Trusted startup configuration, never renderer input or an inherited key. */
export function localProfile(options: {
  packaged: boolean;
  environment: NodeJS.ProcessEnv;
  home: string;
}): { userDataName?: string; sidecarEnv: NodeJS.ProcessEnv } {
  const { environment, home } = options;
  if (options.packaged || environment.AXIOM_DESKTOP_LOCAL_PROFILE !== "1") {
    return { sidecarEnv: {} };
  }
  if (environment.AXIOM_DESKTOP_STAGING_PROFILE === "1") {
    throw new Error("Choose one development profile");
  }
  const origins = resolveServiceOrigins({ packaged: false,
    apiUrl: environment.AXIOM_BASE_URL, authUrl: environment.AXIOM_AUTH_URL });
  const loopback = new Set(["localhost", "127.0.0.1", "[::1]"]);
  if (!loopback.has(new URL(origins.api).hostname) || !loopback.has(new URL(origins.auth).hostname)) {
    throw new Error("Local profile requires loopback API and auth origins");
  }
  const sidecarEnv: NodeJS.ProcessEnv = {
    XDG_CONFIG_HOME: join(home, ".axiom-local", "config"),
    XDG_DATA_HOME: join(home, ".axiom-local", "data"),
  };
  if (environment.AXIOM_DESKTOP_LOCAL_CREDENTIALS !== undefined) {
    if (environment.AXIOM_DESKTOP_LOCAL_CREDENTIALS !== "1") throw new Error("Invalid local credential mode");
    sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS = "1";
  }
  for (const name of ["AXIOM_POOL_CLIENT_CONFIG_FILE", "AXIOM_LOCAL_RELAY_CA_FILE", "AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG", "AXIOM_CHUTES_CLIENT_CONFIG_FILE"] as const) {
    const path = environment[name];
    if (path !== undefined) {
      if (!isAbsolute(path)) throw new Error(`${name} requires an absolute path`);
      sidecarEnv[name] = path;
    }
  }
  return { userDataName: "Axiom Local", sidecarEnv };
}
