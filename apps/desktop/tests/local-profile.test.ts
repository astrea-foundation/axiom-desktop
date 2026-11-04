import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { localProfile } from "../src/main/local-profile";

const home = resolve("test-user-profile");
const environment = {
  AXIOM_DESKTOP_LOCAL_PROFILE: "1",
  AXIOM_AUTH_URL: "http://localhost:4174",
  AXIOM_BASE_URL: "https://127.0.0.1:9443",
  AXIOM_POOL_CLIENT_CONFIG_FILE: join(home, "pool.json"),
  AXIOM_LOCAL_RELAY_CA_FILE: join(home, "local-ca.pem"),
  AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG: join(home, "native-preview.json"),
  CHUTES_API_KEY: "must-not-reach-sidecar",
};

test("named hosted staging profiles isolate all storage and forward public account pins only", () => {
  const staging = { AXIOM_DESKTOP_STAGING_PROFILE: "1", AXIOM_DESKTOP_STAGING_PROFILE_NAME: "astreaai-qa-20261104",
    AXIOM_BASE_URL: "https://api-staging.axiom.stream", AXIOM_AUTH_URL: "https://auth-staging.axiom.stream",
    AXIOM_DIRECT_CLIENT_CONFIG_FILE: join(home, "astreaai-public.json"),
    AXIOM_LOCAL_RELAY_CA_FILE: join(home, "must-not-pass.pem"),
    AXIOM_DESKTOP_LOCAL_CREDENTIALS: "1", VERDA_CLIENT_SECRET: "must-not-pass" };
  const result = localProfile({ packaged: false, environment: staging, home });
  assert.equal(result.userDataName, "Axiom Staging astreaai-qa-20261104");
  assert.deepEqual(result.sidecarEnv, {
    XDG_CONFIG_HOME: join(home, ".axiom-staging", "profiles", "astreaai-qa-20261104", "config"),
    XDG_DATA_HOME: join(home, ".axiom-staging", "profiles", "astreaai-qa-20261104", "data"),
    AXIOM_DIRECT_CLIENT_CONFIG_FILE: staging.AXIOM_DIRECT_CLIENT_CONFIG_FILE,
  });
  assert.deepEqual(localProfile({ packaged: true, environment: staging, home }), { sidecarEnv: {} });
  for (const replacement of [
    { AXIOM_DESKTOP_STAGING_PROFILE_NAME: "../existing" },
    { AXIOM_DIRECT_CLIENT_CONFIG_FILE: "relative.json" },
    { AXIOM_BASE_URL: "https://api.axiom.stream", AXIOM_AUTH_URL: "https://auth.axiom.stream" },
  ]) assert.throws(() => localProfile({ packaged: false, environment: { ...staging, ...replacement }, home }));
  const defaultProfile = localProfile({ packaged: false, environment: { ...staging, AXIOM_DESKTOP_STAGING_PROFILE_NAME: undefined }, home });
  assert.equal(defaultProfile.userDataName, "Axiom Staging");
  assert.equal(defaultProfile.sidecarEnv.XDG_DATA_HOME, join(home, ".axiom-staging", "data"));
});

test("local inference profile isolates storage and passes only public trust configuration", () => {
  const result = localProfile({ packaged: false, environment, home });
  assert.equal(result.userDataName, "Axiom Local");
  assert.deepEqual(result.sidecarEnv, {
    XDG_CONFIG_HOME: join(home, ".axiom-local", "config"),
    XDG_DATA_HOME: join(home, ".axiom-local", "data"),
    AXIOM_POOL_CLIENT_CONFIG_FILE: environment.AXIOM_POOL_CLIENT_CONFIG_FILE,
    AXIOM_LOCAL_RELAY_CA_FILE: environment.AXIOM_LOCAL_RELAY_CA_FILE,
    AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG: environment.AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG,
  });
});

test("packaged or ordinary profiles cannot inherit local trust overrides", () => {
  assert.deepEqual(localProfile({ packaged: true, environment, home: "/test-user" }), { sidecarEnv: {} });
  assert.deepEqual(localProfile({ packaged: false, environment: { ...environment, AXIOM_DESKTOP_LOCAL_PROFILE: "0" }, home: "/test-user" }), { sidecarEnv: {} });
});

test("named QA profiles isolate native databases, credentials and Electron storage", () => {
  const named = {...environment, AXIOM_DESKTOP_LOCAL_PROFILE_NAME:"verda-images-qa"};
  const result = localProfile({packaged:false,environment:named,home});
  assert.equal(result.userDataName,"Axiom Local verda-images-qa");
  assert.equal(result.sidecarEnv.XDG_DATA_HOME,join(home,".axiom-local","profiles","verda-images-qa","data"));
  assert.equal(result.sidecarEnv.XDG_CONFIG_HOME,join(home,".axiom-local","profiles","verda-images-qa","config"));
  for (const name of ["", "../other", "/absolute", "mixed/parts", "a".repeat(49)]) {
    assert.throws(()=>localProfile({packaged:false,environment:{...named,AXIOM_DESKTOP_LOCAL_PROFILE_NAME:name},home}));
  }
  assert.deepEqual(localProfile({packaged:true,environment:named,home}),{sidecarEnv:{}});
});

test("local credentials are an explicit override and never inherited by packaged or ordinary profiles", () => {
  const optedIn = {...environment, AXIOM_DESKTOP_LOCAL_CREDENTIALS: "1"};
  assert.equal(localProfile({packaged:false, environment:optedIn, home}).sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS, "1");
  assert.equal(localProfile({packaged:true, environment:optedIn, home}).sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS, undefined);
  assert.equal(localProfile({packaged:false, environment:{...optedIn,AXIOM_DESKTOP_LOCAL_PROFILE:"0"},home}).sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS, undefined);
  assert.equal(localProfile({packaged:false, environment:{...environment,AXIOMCLI_LOCAL_DEV_CREDENTIALS:"1"},home}).sidecarEnv.AXIOMCLI_LOCAL_DEV_CREDENTIALS, undefined);
  assert.throws(()=>localProfile({packaged:false,environment:{...environment,AXIOM_DESKTOP_LOCAL_CREDENTIALS:"true"},home}));
});

test("local profile rejects remote audiences, mixed profiles and relative trust paths", () => {
  for (const replacement of [
    { AXIOM_BASE_URL: "https://api.axiom.stream", AXIOM_AUTH_URL: "https://auth.axiom.stream" },
    { AXIOM_DESKTOP_STAGING_PROFILE: "1" },
    { AXIOM_POOL_CLIENT_CONFIG_FILE: "relative.json" },
    { AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG: "relative.json" },
  ]) {
    assert.throws(() => localProfile({ packaged: false, environment: { ...environment, ...replacement }, home: "/test-user" }));
  }
});
