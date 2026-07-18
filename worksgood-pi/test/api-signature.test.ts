/**
 * Compile-time guard for the pi 0.79.x extension API shape WG's plugin relies on.
 *
 * This file intentionally assigns the upstream methods to exact function types.
 * If ExtensionAPI, setModel(), or registerProvider() drift, `npm run build`
 * fails before the smoke gate reaches live provider validation.
 */

import { describe, expect, it } from "vitest";
import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import type { Model } from "@earendil-works/pi-ai";

type Assert<T extends true> = T;
type IsExact<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2
  ? (<T>() => T extends B ? 1 : 2) extends <T>() => T extends A ? 1 : 2
    ? true
    : false
  : false;

type SetModelSignature = ExtensionAPI["setModel"];
type RegisterProviderSignature = ExtensionAPI["registerProvider"];

type _SetModelPinned = Assert<IsExact<SetModelSignature, (model: Model<any>) => Promise<boolean>>>;
type _RegisterProviderPinned = Assert<
  IsExact<RegisterProviderSignature, (name: string, config: ProviderConfig) => void>
>;

describe("pi ExtensionAPI signature guard", () => {
  it("pins setModel/registerProvider to the pi 0.79.x shape", () => {
    expect(true).toBe(true);
  });
});
