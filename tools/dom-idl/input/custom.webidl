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
