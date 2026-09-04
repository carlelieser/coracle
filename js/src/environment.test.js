import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  assertBuildMatchesEnvironment,
  canRunThreadedBuild,
  CROSS_ORIGIN_ISOLATION_HEADERS,
  selectBuildVariant,
} from "./environment.js";

/**
 * @param {{ hasSharedArrayBuffer?: boolean, isIsolated?: boolean }} options
 * @returns {typeof globalThis}
 */
function fakeScope({ hasSharedArrayBuffer = false, isIsolated = false }) {
  return /** @type {typeof globalThis} */ ({
    SharedArrayBuffer: hasSharedArrayBuffer ? function () {} : undefined,
    crossOriginIsolated: isIsolated,
  });
}

const threadedBuild = { threaded: true, requiresCrossOriginIsolation: true };
const degradedBuild = { threaded: false, requiresCrossOriginIsolation: false };

describe("canRunThreadedBuild", () => {
  it("accepts a cross-origin isolated page that exposes SharedArrayBuffer", () => {
    const scope = fakeScope({ hasSharedArrayBuffer: true, isIsolated: true });

    assert.equal(canRunThreadedBuild(scope), true);
  });

  it("rejects an isolated page with no SharedArrayBuffer", () => {
    const scope = fakeScope({ hasSharedArrayBuffer: false, isIsolated: true });

    assert.equal(canRunThreadedBuild(scope), false);
  });

  it("rejects SharedArrayBuffer on a page that is not isolated", () => {
    // The constructor can exist while the engine still refuses to back a
    // WebAssembly.Memory with it, so the isolation flag decides.
    const scope = fakeScope({ hasSharedArrayBuffer: true, isIsolated: false });

    assert.equal(canRunThreadedBuild(scope), false);
  });
});

describe("selectBuildVariant", () => {
  it("picks the threaded build only when the page can host it", () => {
    const isolated = fakeScope({ hasSharedArrayBuffer: true, isIsolated: true });
    const plain = fakeScope({});

    assert.equal(selectBuildVariant(isolated), "threaded");
    assert.equal(selectBuildVariant(plain), "single-threaded");
  });
});

describe("assertBuildMatchesEnvironment", () => {
  it("rejects a threaded module loaded into a non-isolated page", () => {
    // Without this the worker reaches Atomics.wait and hangs silently.
    assert.throws(
      () => assertBuildMatchesEnvironment(threadedBuild, fakeScope({})),
      /cross-origin isolated/,
    );
  });

  it("accepts a threaded module on an isolated page", () => {
    const scope = fakeScope({ hasSharedArrayBuffer: true, isIsolated: true });

    assert.doesNotThrow(() => assertBuildMatchesEnvironment(threadedBuild, scope));
  });

  it("accepts the degraded module anywhere, which is why it exists", () => {
    assert.doesNotThrow(() =>
      assertBuildMatchesEnvironment(degradedBuild, fakeScope({})),
    );
  });
});

describe("CROSS_ORIGIN_ISOLATION_HEADERS", () => {
  it("names the exact headers a host page must serve", () => {
    assert.deepEqual(
      { ...CROSS_ORIGIN_ISOLATION_HEADERS },
      {
        "Cross-Origin-Opener-Policy": "same-origin",
        "Cross-Origin-Embedder-Policy": "require-corp",
      },
    );
  });

  it("cannot be mutated by a consumer", () => {
    assert.throws(() => {
      "use strict";
      CROSS_ORIGIN_ISOLATION_HEADERS["Cross-Origin-Opener-Policy"] = "unsafe-none";
    });
  });
});
