partial interface EventTarget {
  undefined addEventListener(DOMString type, EventListener callback);
};

partial interface Event {
  readonly attribute EventTarget target;
};

partial interface Node {
  attribute DOMString textContent;
};

partial interface Element {
  undefined setAttribute(DOMString qualifiedName, DOMString value);
};

partial interface HTMLElement {
  attribute DOMString value;
};

partial interface Document {
  readonly attribute HTMLElement body;
  readonly attribute Element documentElement;
};

// Waluau-curated GPU canvas additions (waluau-9tvw). BufferSource is the
// extern face of a host JS typed-array view over the module's linear memory,
// produced by the dom_float32_array_view host function and consumed by
// BufferSource-typed WebGL params like bufferData. getContextWebGL2 is the
// curated 3D context acquisition path while getContext keeps its
// Canvas-2D-only signature override.
interface BufferSource {
};

partial interface HTMLCanvasElement {
  WebGL2RenderingContext? getContextWebGL2();
};
