import { readFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "..");

describe("WorksGood Pi identity", () => {
  it("uses the canonical npm metadata and Pi display path", async () => {
    const pkg = JSON.parse(await readFile(resolve(root, "package.json"), "utf8"));

    expect(pkg.name).toBe("@worksgood/pi");
    expect(pkg.description).toBe(
      "Connect Pi agents to WorksGood graphs, tools, and context.",
    );
    expect(pkg.keywords).toContain("pi-package");
    expect(pkg.pi.extensions).toEqual(["./pi-worksgood/index.js"]);

    // Pi's startup list strips index.js and displays its parent directory for
    // path-loaded extensions. This pins the public label independently of npm.
    const extensionEntry = pkg.pi.extensions[0];
    expect(basename(dirname(extensionEntry))).toBe("pi-worksgood");
    expect(basename(extensionEntry)).toBe("index.js");
  });

  it("keeps package-lock identity in lockstep", async () => {
    const lock = JSON.parse(await readFile(resolve(root, "package-lock.json"), "utf8"));
    expect(lock.name).toBe("@worksgood/pi");
    expect(lock.packages[""].name).toBe("@worksgood/pi");
  });
});
