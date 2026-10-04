/**
 * The regression invariant from the crash fix: a `handleMouse` return value is
 * always falsy or a non-null object, never a truthy primitive. Centralised so
 * the contract test and any future panel share one definition.
 */
export function isPiMouseResult(value) {
    return (value === undefined ||
        value === false ||
        (typeof value === "object" && value !== null));
}
//# sourceMappingURL=mouse.js.map