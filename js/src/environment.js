/**
 * Which of the two wasm builds the current environment can run.
 *
 * The threaded build needs a SharedArrayBuffer, which browsers expose only to
 * cross-origin isolated pages (COOP: same-origin, COEP: require-corp). Where
 * that is unavailable — Safari quirks, or an embedding page that cannot set the
 * headers — the single-threaded build runs instead, at reduced speed.
 */

/** @typedef {"threaded" | "single-threaded"} BuildVariant */

/**
 * Detects whether this page can host the threaded build.
 *
 * `crossOriginIsolated` is the browser's own answer to "may I use
 * SharedArrayBuffer", so it is checked rather than inferred from the headers.
 * Both checks are needed: some engines define the constructor while still
 * refusing to let it back a WebAssembly.Memory on a non-isolated page.
 *
 * @param {typeof globalThis} [scope]
 * @returns {boolean}
 */
export function canRunThreadedBuild(scope = globalThis) {
  const hasSharedArrayBuffer = typeof scope.SharedArrayBuffer === "function";
  const isIsolated = scope.crossOriginIsolated === true;

  return hasSharedArrayBuffer && isIsolated;
}

/**
 * Picks the build variant to load.
 *
 * @param {typeof globalThis} [scope]
 * @returns {BuildVariant}
 */
export function selectBuildVariant(scope = globalThis) {
  return canRunThreadedBuild(scope) ? "threaded" : "single-threaded";
}

/**
 * Fails when a loaded module disagrees with the environment it was loaded into.
 *
 * Called after instantiating the wasm module, against the `buildInfo()` it
 * exports. Without this check a threaded module on a non-isolated page reaches
 * `Atomics.wait` on the main thread and hangs with no diagnostic.
 *
 * @param {{ threaded: boolean, requiresCrossOriginIsolation: boolean }} buildInfo
 * @param {typeof globalThis} [scope]
 * @returns {void}
 */
export function assertBuildMatchesEnvironment(buildInfo, scope = globalThis) {
  if (!buildInfo.requiresCrossOriginIsolation) {
    return;
  }
  if (canRunThreadedBuild(scope)) {
    return;
  }

  throw new Error(
    "load emulator module: the threaded build requires a cross-origin isolated " +
      "page (COOP: same-origin, COEP: require-corp). Serve those headers, or " +
      "load the single-threaded build.",
  );
}

/**
 * The response headers a page must serve to host the threaded build.
 *
 * Exported as data so the dev server, the test harness, and the deployment docs
 * cannot drift from one another.
 *
 * @type {Readonly<Record<string, string>>}
 */
export const CROSS_ORIGIN_ISOLATION_HEADERS = Object.freeze({
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
});
