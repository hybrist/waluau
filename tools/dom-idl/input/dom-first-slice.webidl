interface EventTarget {
  undefined addEventListener(DOMString type, EventListener callback);
};

interface Node : EventTarget {
  readonly attribute DOMString nodeName;
  attribute DOMString textContent;
  Node appendChild(Node node);
  Node replaceChild(Node node, Node child);
  Node removeChild(Node child);
};

interface Document : Node {
  readonly attribute HTMLElement body;
  readonly attribute Element documentElement;
  Element createElement(DOMString localName);
  Element? getElementById(DOMString elementId);
  Element? querySelector(DOMString selectors);
};

interface Window {
  readonly attribute Document document;
  readonly attribute Storage localStorage;
};

interface Element : Node {
  attribute DOMString id;
  attribute DOMString className;
  DOMString? getAttribute(DOMString qualifiedName);
  undefined setAttribute(DOMString qualifiedName, DOMString value);
  undefined removeAttribute(DOMString qualifiedName);
  undefined appendClass(DOMString className);
  Element? querySelector(DOMString selectors);
};

interface HTMLElement : Element {
  attribute DOMString innerText;
  attribute DOMString value;
};

interface HTMLHeadingElement : HTMLElement {
};

interface HTMLInputElement : HTMLElement {
};

interface HTMLTextAreaElement : HTMLElement {
};

interface Storage {
  DOMString? getItem(DOMString key);
  undefined setItem(DOMString key, DOMString value);
  undefined removeItem(DOMString key);
};
