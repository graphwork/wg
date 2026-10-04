/**
 * mouse.ts — pi-tui's mouse-dispatch return contract for custom components.
 *
 * pi's `dispatchMouseEvent` (bundle `chunk-6FX7UEPL.js`) is:
 *
 * ```js
 * function dispatchMouseEvent(component, event) {
 *   let result = component.handleMouse?.(event);
 *   if (result) {
 *     if ("target" in result) { ...forward... }
 *     if (!(!result.handled && !result.capture && !result.focus)) {
 *       return { ...result, handled: true, ..., target: {...} };
 *     }
 *   }
 * }
 * ```
 *
 * So `handleMouse` may return a FALSY value (`undefined` / `false`) meaning
 * "not mine", or a TRUTHY OBJECT with optional `{handled, capture, focus,
 * target}`. Returning a truthy NON-object (e.g. a bare `true`) makes pi evaluate
 * the `in` operator against a boolean and throw
 * `TypeError: Cannot use 'in' operator to search for 'target' in true`,
 * crashing pi core. Every panel's `handleMouse` must therefore return this
 * shape — never a bare boolean.
 */
export type PiMouseResult =
  | undefined
  | false
  | {
      /** The event was consumed by this component. */
      handled?: boolean;
      /** Keep receiving drag/pointer sequences (capture the gesture). */
      capture?: boolean;
      /** Ask the host to focus this component. */
      focus?: boolean;
      /** Pre-targeted result forwarded verbatim (advanced; rarely needed). */
      target?: unknown;
    };

/**
 * The regression invariant from the crash fix: a `handleMouse` return value is
 * always falsy or a non-null object, never a truthy primitive. Centralised so
 * the contract test and any future panel share one definition.
 */
export function isPiMouseResult(value: unknown): boolean {
  return (
    value === undefined ||
    value === false ||
    (typeof value === "object" && value !== null)
  );
}
