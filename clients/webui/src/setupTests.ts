/*
 * jsdom does not implement ResizeObserver, which Fluent's MessageBar and
 * Overflow primitives observe on mount. A no-op stub is enough: the layout
 * they compute is never asserted in tests.
 */
class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
}

if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = ResizeObserverStub;
}
