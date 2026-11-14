const RESET_ERROR = "--axiom-factory-reset-error=";

/** Keep a dev restart self-contained, including the renderer server. */
export function electronLaunch(argv, environment, executable, entry) {
  const forwarded = argv.filter((argument) => argument.startsWith(RESET_ERROR));
  const [target, ...args] = argv.filter((argument) => !argument.startsWith(RESET_ERROR));
  const env = { ...environment };
  delete env.ELECTRON_RUN_AS_NODE;
  // A reset helper inherits the old server's URL. The new Vite process must
  // choose and publish its own URL, or use built files in preview mode.
  delete env.ELECTRON_RENDERER_URL;
  delete env.AXIOM_DESKTOP_DEV_RESTART;
  if ((target === "vite" || target === "staging") && args[0] === "dev") {
    env.AXIOM_DESKTOP_DEV_RESTART = JSON.stringify({ executable, args: [entry, target, ...args] });
  }
  const electronArgs = JSON.parse(env.ELECTRON_CLI_ARGS || "[]");
  if (!Array.isArray(electronArgs) || !electronArgs.every((argument) => typeof argument === "string")) {
    throw new Error("ELECTRON_CLI_ARGS must be a JSON array of strings");
  }
  // A native helper appends a failure to the launcher argv. Forward it to
  // Electron, not Vite, and don't retain an older failure on a later reset.
  env.ELECTRON_CLI_ARGS = JSON.stringify([
    ...electronArgs.filter((argument) => !argument.startsWith(RESET_ERROR)),
    ...forwarded,
  ]);
  return { target, args, env };
}
