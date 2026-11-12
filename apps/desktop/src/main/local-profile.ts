import { isAbsolute, join } from "node:path";
import { resolveServiceOrigins, STAGING_API_ORIGIN, STAGING_AUTH_ORIGIN } from "./service-origins";

/** Trusted startup configuration, never renderer input or an inherited key. */
export function localProfile(options: {
  packaged: boolean;
  environment: NodeJS.ProcessEnv;
  home: string;
}): { userDataName?: string; sidecarEnv: NodeJS.ProcessEnv } {
  const { environment, home } = options;
  if (options.packaged) {
    return { sidecarEnv: {} };
  }
  if (environment.AXIOM_DESKTOP_STAGING_PROFILE === "1" && environment.AXIOM_DESKTOP_LOCAL_PROFILE !== "1") {
    const origins = resolveServiceOrigins({ packaged: false,
      apiUrl: environment.AXIOM_BASE_URL, authUrl: environment.AXIOM_AUTH_URL });
    if (origins.api !== STAGING_API_ORIGIN || origins.auth !== STAGING_AUTH_ORIGIN) {
      throw new Error("Staging profile requires staging service origins");
    }
    const name = environment.AXIOM_DESKTOP_STAGING_PROFILE_NAME;
    if (name !== undefined && !/^[a-z0-9][a-z0-9-]{0,47}$/.test(name)) throw new Error("Invalid staging profile name");
    const root = name ? join(home, ".axiom-staging", "profiles", name) : join(home, ".axiom-staging");
    const sidecarEnv: NodeJS.ProcessEnv = { XDG_CONFIG_HOME: join(root, "config"), XDG_DATA_HOME: join(root, "data") };
    for (const key of ["AXIOM_DIRECT_CLIENT_CONFIG_FILE", "AXIOM_POOL_CLIENT_CONFIG_FILE"] as const) {
      const path = environment[key];
      if (path !== undefined) {
        if (!isAbsolute(path)) throw new Error(`${key} requires an absolute path`);
        sidecarEnv[key] = path;
      }
    }
    return { userDataName: name ? `Axiom Staging ${name}` : "Axiom Staging", sidecarEnv };
  }
  if (environment.AXIOM_DESKTOP_LOCAL_PROFILE !== "1") return { sidecarEnv: {} };
  if (environment.AXIOM_DESKTOP_STAGING_PROFILE === "1") {
    throw new Error("Choose one development profile");
  }
  const origins = resolveServiceOrigins({ packaged: false,
    apiUrl: environment.AXIOM_BASE_URL, authUrl: environment.AXIOM_AUTH_URL });
  const loopback = new Set(["localhost", "127.0.0.1", "[::1]"]);
  if (!loopback.has(new URL(origins.api).hostname) || !loopback.has(new URL(origins.auth).hostname)) {
    throw new Error("Local profile requires loopback API and auth origins");
  }
  const name = environment.AXIOM_DESKTOP_LOCAL_PROFILE_NAME;
  if (name !== undefined && !/^[a-z0-9][a-z0-9-]{0,47}$/.test(name)) {
    throw new Error("Invalid local profile name");
  }
  const root = name ? join(home, ".axiom-local", "profiles", name) : join(home, ".axiom-local");
  const sidecarEnv: NodeJS.ProcessEnv = {
    XDG_CONFIG_HOME: join(root, "config"),
    XDG_DATA_HOME: join(root, "data"),
  };
  if (environment.AXIOM_DESKTOP_LOCAL_CREDENTIALS !== undefined) {
    if (environment.AXIOM_DESKTOP_LOCAL_CREDENTIALS !== "1") throw new Error("Invalid local credential mode");
    sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS = "1";
  }
  for (const name of ["AXIOM_POOL_CLIENT_CONFIG_FILE", "AXIOM_DIRECT_CLIENT_CONFIG_FILE", "AXIOM_LOCAL_RELAY_CA_FILE"] as const) {
    const path = environment[name];
    if (path !== undefined) {
      if (!isAbsolute(path)) throw new Error(`${name} requires an absolute path`);
      sidecarEnv[name] = path;
    }
  }
  return { userDataName: name ? `Axiom Local ${name}` : "Axiom Local", sidecarEnv };
}
