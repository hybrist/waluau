interface EventTarget {
  undefined addEventListener(DOMString type, EventListener callback);
};

interface Node : EventTarget {
  readonly attribute DOMString nodeName;
  attribute DOMString textContent;
  Node appendChild(Node node);
};

interface Document : Node {
  Element createElement(DOMString localName);
  Element? getElementById(DOMString elementId);
  Element? querySelector(DOMString selectors);
};

interface Element : Node {
  attribute DOMString id;
  attribute DOMString className;
  Element? querySelector(DOMString selectors);
};

interface HTMLElement : Element {
  attribute DOMString innerText;
};

interface HTMLHeadingElement : HTMLElement {
};
