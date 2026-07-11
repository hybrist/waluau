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

// Waluau-curated GPU canvas additions (waluau-9tvw). Float32Buffer is the
// extern face of a host JS Float32Array (Float32Array itself is a reserved
// Web IDL type name), constructed through the dom_float32_buffer_* host
// functions. getContextWebGL2 is the curated 3D context acquisition path
// while getContext keeps its Canvas-2D-only signature override.
interface Float32Buffer {
};

partial interface HTMLCanvasElement {
  WebGL2RenderingContext? getContextWebGL2();
};
