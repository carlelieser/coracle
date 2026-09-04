/**
 * Public SDK surface.
 *
 * Skeleton only. `Box.pull`, `vm.exec` and the rest of the 1.0 API arrive in
 * M4/M6; what exists now is the environment handshake that decides which wasm
 * build to load.
 */

export {
  assertBuildMatchesEnvironment,
  canRunThreadedBuild,
  CROSS_ORIGIN_ISOLATION_HEADERS,
  selectBuildVariant,
} from "./environment.js";
