const WALUAU_STRING_CONSTANTS_MODULE = 'string_constants';
const WALUAU_IMPORT_MODULE = 'waluau';

function decodeBytesConstantsFromWasm(wasmBuffer) {
  const bytes = wasmBuffer instanceof Uint8Array ? wasmBuffer : new Uint8Array(wasmBuffer);
  let pos = 8;

  function readVaruint() {
    let result = 0;
    let shift = 0;
    while (pos < bytes.length) {
      const byte = bytes[pos++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return result >>> 0;
      shift += 7;
    }
    throw new Error('truncated wasm leb128');
  }

  function readU32Le(offset) {
    return (
      bytes[offset] |
      (bytes[offset + 1] << 8) |
      (bytes[offset + 2] << 16) |
      (bytes[offset + 3] << 24)
    ) >>> 0;
  }

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionStart = pos;
    const sectionEnd = sectionStart + sectionLength;
    if (sectionEnd > bytes.length) throw new Error('wasm section extends past end of module');

    if (sectionId === 0) {
      const nameLen = readVaruint();
      const nameStart = pos;
      const nameEnd = nameStart + nameLen;
      const name = new TextDecoder().decode(bytes.subarray(nameStart, nameEnd));
      pos = nameEnd;
      if (name === 'waluau.bytc') {
        let offset = pos;
        const count = readU32Le(offset);
        offset += 4;
        const values = [];
        for (let i = 0; i < count; i++) {
          const len = readU32Le(offset);
          offset += 4;
          values.push(bytes.slice(offset, offset + len));
          offset += len;
        }
        return values;
      }
    }

    pos = sectionEnd;
  }

  return [];
}

function createMockElement(tagName) {
  const attributes = new Map();
  const listeners = new Map();
  const element = {
    tagName: String(tagName).toUpperCase(),
    id: '',
    className: '',
    childNodes: [],
    parentNode: null,
    textContent: '',
    value: '',
    attributes,
    classList: {
      add(...tokens) {
        const existing = new Set(String(this.owner.className).split(/\s+/).filter(Boolean));
        for (const token of tokens) {
          const normalized = String(token).trim();
          if (normalized) existing.add(normalized);
        }
        this.owner.className = Array.from(existing).join(' ');
      },
      owner: null,
    },
    appendChild(child) {
      if (child.parentNode?.removeChild) child.parentNode.removeChild(child);
      this.childNodes.push(child);
      child.parentNode = this;
      return child;
    },
    replaceChild(node, child) {
      const index = this.childNodes.indexOf(child);
      if (index < 0) throw new Error('child not found');
      if (node.parentNode?.removeChild) node.parentNode.removeChild(node);
      this.childNodes[index] = node;
      node.parentNode = this;
      child.parentNode = null;
      return child;
    },
    removeChild(child) {
      const index = this.childNodes.indexOf(child);
      if (index < 0) throw new Error('child not found');
      this.childNodes.splice(index, 1);
      child.parentNode = null;
      return child;
    },
    replaceChildren(...children) {
      for (const child of this.childNodes) child.parentNode = null;
      this.childNodes = [];
      for (const child of children) this.appendChild(child);
    },
    setAttribute(name, value) {
      const normalized = String(name);
      attributes.set(normalized, String(value));
      if (normalized === 'id') this.id = String(value);
      if (normalized === 'class') this.className = String(value);
      if (normalized === 'value') this.value = String(value);
    },
    getAttribute(name) {
      const normalized = String(name);
      if (normalized === 'id') return this.id || null;
      if (normalized === 'class') return this.className || null;
      return attributes.has(normalized) ? attributes.get(normalized) : null;
    },
    removeAttribute(name) {
      const normalized = String(name);
      attributes.delete(normalized);
      if (normalized === 'id') this.id = '';
      if (normalized === 'class') this.className = '';
    },
    querySelector(selector) {
      return queryMockSelector(this, String(selector));
    },
    addEventListener(type, listener) {
      const eventType = String(type);
      let listenersForType = listeners.get(eventType);
      if (!listenersForType) {
        listenersForType = new Set();
        listeners.set(eventType, listenersForType);
      }
      listenersForType.add(listener);
    },
    removeEventListener(type, listener) {
      const listenersForType = listeners.get(String(type));
      if (listenersForType) listenersForType.delete(listener);
    },
    dispatchEvent(event) {
      const eventObject = event && typeof event === 'object' ? event : { type: String(event) };
      if (!eventObject.type) throw new Error('mock DOM event requires a type');
      if (!('target' in eventObject)) eventObject.target = this;
      const listenersForType = Array.from(listeners.get(String(eventObject.type)) ?? []);
      for (const listener of listenersForType) listener(eventObject);
      return true;
    },
    get children() {
      return this.childNodes;
    },
    get innerHTML() {
      const ownText = escapeHtml(this.textContent);
      const children = this.childNodes.map((child) => child.outerHTML).join('');
      return `${ownText}${children}`;
    },
    get outerHTML() {
      const tag = this.tagName.toLowerCase();
      return `<${tag}>${this.innerHTML}</${tag}>`;
    },
  };
  element.classList.owner = element;
  return element;
}

function queryMockSelector(root, selector) {
  const matches = (node) => {
    if (!node || typeof node !== 'object') return false;
    if (selector.startsWith('#')) return node.id === selector.slice(1);
    if (selector.startsWith('.')) {
      return String(node.className).split(/\s+/).includes(selector.slice(1));
    }
    return String(node.tagName).toLowerCase() === selector.toLowerCase();
  };
  for (const child of Array.from(root.children ?? root.childNodes ?? [])) {
    if (matches(child)) return child;
    const found = queryMockSelector(child, selector);
    if (found) return found;
  }
  return null;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function createMockStorage() {
  const values = new Map();
  return {
    getItem(key) {
      const normalized = String(key);
      return values.has(normalized) ? values.get(normalized) : null;
    },
    setItem(key, value) {
      values.set(String(key), String(value));
    },
    removeItem(key) {
      values.delete(String(key));
    },
  };
}

function createDomHost() {
  const storage = (() => {
    try {
      if (globalThis.localStorage?.getItem) return globalThis.localStorage;
    } catch {
      // Fall back below for restricted browser contexts.
    }
    return createMockStorage();
  })();

  if (globalThis.document?.createElement) {
    const root = globalThis.document.createElement('div');
    root.dataset.waluauConformanceRoot = '';
    root.id = 'waluau-conformance-root';
    return {
      document: root,
      window: root.ownerDocument.defaultView,
      createElement: (tagName) => globalThis.document.createElement(tagName),
      storage,
      root,
    };
  }

  const root = createMockElement('div');
  root.id = 'waluau-conformance-root';
  return {
    document: root,
    window: { document: root, localStorage: storage },
    createElement: (tagName) => createMockElement(tagName),
    storage,
    root,
  };
}

function buildWaluauImports(wasmBuffer, options = {}) {
  const bytesConstants = decodeBytesConstantsFromWasm(wasmBuffer);
  const domHost = options.domHost ?? createDomHost();
  const hostImports = options.hostImports ?? {};
  const getWasmExports = options.getWasmExports ?? (() => null);
  const eventListeners = new WeakMap();
  const asBytes = (value) => {
    if (value instanceof Uint8Array) return value;
    throw new Error(`Expected Uint8Array bytes value, got ${Object.prototype.toString.call(value)}`);
  };
  const asElement = (value, name) => {
    if (value && typeof value.appendChild === 'function') return value;
    throw new Error(`Expected DOM Element for ${name}`);
  };
  const asNode = (value, name) => {
    if (value && typeof value === 'object' && typeof value.appendChild === 'function') return value;
    throw new Error(`Expected DOM Node for ${name}`);
  };
  const asEventTarget = (value, name) => {
    if (value && typeof value.addEventListener === 'function' && typeof value.removeEventListener === 'function') return value;
    throw new Error(`Expected DOM EventTarget for ${name}`);
  };
  const asEvent = (value, name) => {
    if (value && typeof value === 'object' && 'target' in value) return value;
    throw new Error(`Expected DOM Event for ${name}`);
  };
  const asStorage = (value, name) => {
    if (value && typeof value.getItem === 'function' && typeof value.setItem === 'function' && typeof value.removeItem === 'function') return value;
    throw new Error(`Expected Storage for ${name}`);
  };
  const documentBody = (document, name) => {
    if (document !== domHost.document) throw new Error(`Expected DOM Document for ${name}`);
    return domHost.root;
  };
  const validateAttrName = (name) => {
    const attrName = String(name).trim().toLowerCase();
    if (!/^[a-z_:][a-z0-9_:.-]*$/.test(attrName) || attrName.startsWith('on')) {
      throw new Error(`Unsupported DOM attribute: ${name}`);
    }
    return attrName;
  };
  const setText = (element, text, name) => {
    asElement(element, name).textContent = String(text);
  };
  const appendChild = (parent, child, name) => {
    return asNode(parent, name).appendChild(asNode(child, name));
  };
  const replaceChild = (parent, newChild, oldChild, name) => {
    return asNode(parent, name).replaceChild(asNode(newChild, name), asNode(oldChild, name));
  };
  const removeChild = (parent, child, name) => {
    return asNode(parent, name).removeChild(asNode(child, name));
  };
  const childrenOf = (node) => Array.from(node.children ?? node.childNodes ?? []);
  const removeRegisteredListeners = (target) => {
    const records = eventListeners.get(target);
    if (!records) return;
    for (const { type, listener } of records) {
      target.removeEventListener(type, listener);
    }
    records.clear();
    eventListeners.delete(target);
  };
  const cleanupEventListeners = (node) => {
    if (!node || typeof node !== 'object') return;
    for (const child of childrenOf(node)) cleanupEventListeners(child);
    if (typeof node.removeEventListener === 'function') removeRegisteredListeners(node);
  };
  const registerEventListener = (target, type, callback, name) => {
    const eventTarget = asEventTarget(target, name);
    const eventType = String(type);
    if (eventType !== 'click' && eventType !== 'input') {
      throw new Error(`${name} only supports click and input events`);
    }
    const listener = (event) => {
      const exports = getWasmExports();
      const trampoline = exports?.__waluau_call_callback_event_unit;
      if (typeof trampoline !== 'function') {
        throw new Error('Missing __waluau_call_callback_event_unit export for DOM event callback');
      }
      trampoline(callback, event);
    };
    eventTarget.addEventListener(eventType, listener);
    let records = eventListeners.get(eventTarget);
    if (!records) {
      records = new Set();
      eventListeners.set(eventTarget, records);
    }
    records.add({ type: eventType, listener });
  };
  const walk = (root, visit) => {
    if (visit(root)) return root;
    for (const child of childrenOf(root)) {
      const found = walk(child, visit);
      if (found) return found;
    }
    return null;
  };
  const getElementById = (root, id) => walk(root, (node) => node.id === id);
  const getInnerText = (element, name) => {
    const target = asElement(element, name);
    return 'innerText' in target ? target.innerText : target.textContent;
  };
  const setInnerText = (element, text, name) => {
    const target = asElement(element, name);
    if ('innerText' in target) {
      target.innerText = String(text);
    } else {
      target.textContent = String(text);
    }
  };
  const appendClass = (element, className, name) => {
    const target = asElement(element, name);
    const tokens = String(className).trim().split(/\s+/).filter(Boolean);
    if (tokens.length > 0 && target.classList?.add) {
      target.classList.add(...tokens);
    } else if (tokens.length > 0) {
      const existing = new Set(String(target.className).split(/\s+/).filter(Boolean));
      for (const token of tokens) existing.add(token);
      target.className = Array.from(existing).join(' ');
    }
  };
  const externIs = (value, typeName) => {
    const name = String(typeName);
    if (name === 'Node') {
      return (typeof Node !== 'undefined' && value instanceof Node) || (value && typeof value === 'object' && 'childNodes' in value) ? 1 : 0;
    }
    if (name === 'EventTarget') {
      return (typeof EventTarget !== 'undefined' && value instanceof EventTarget) || (value && typeof value.addEventListener === 'function' && typeof value.removeEventListener === 'function') ? 1 : 0;
    }
    if (name === 'Event') {
      return (typeof Event !== 'undefined' && value instanceof Event) || (value && typeof value === 'object' && 'target' in value) ? 1 : 0;
    }
    if (name === 'Document') {
      return value === domHost.document ? 1 : 0;
    }
    if (name === 'Window') {
      return value === domHost.window ? 1 : 0;
    }
    if (name === 'Element') {
      return (typeof Element !== 'undefined' && value instanceof Element) || (value && typeof value.appendChild === 'function' && typeof value.tagName === 'string') ? 1 : 0;
    }
    if (name === 'HTMLElement') {
      return (typeof HTMLElement !== 'undefined' && value instanceof HTMLElement) || (value && typeof value.appendChild === 'function' && typeof value.tagName === 'string') ? 1 : 0;
    }
    if (name === 'HTMLHeadingElement') {
      return (typeof HTMLHeadingElement !== 'undefined' && value instanceof HTMLHeadingElement) || (value && /^H[1-6]$/.test(String(value.tagName))) ? 1 : 0;
    }
    if (name === 'HTMLInputElement') {
      return (typeof HTMLInputElement !== 'undefined' && value instanceof HTMLInputElement) || (value && String(value.tagName).toUpperCase() === 'INPUT') ? 1 : 0;
    }
    if (name === 'HTMLTextAreaElement') {
      return (typeof HTMLTextAreaElement !== 'undefined' && value instanceof HTMLTextAreaElement) || (value && String(value.tagName).toUpperCase() === 'TEXTAREA') ? 1 : 0;
    }
    if (name === 'Storage') {
      return value === domHost.storage ? 1 : 0;
    }
    throw new Error(`Unsupported extern cast target: ${name}`);
  };
  const waluauImports = new Proxy({}, {
    get(_target, prop) {
      const name = String(prop);
      if (Object.hasOwn(hostImports, name)) {
        return hostImports[name];
      }
      if (name.startsWith('js_tostring_')) {
        return (value) => String(value);
      }
      if (name === 'print' || name === 'js_log') {
        return () => {};
      }
      if (name === 'host_add') {
        return (left, right) => left + right;
      }
      if (name === 'string_find') {
        return (haystack, needle, init, plain) => {
          const hay = String(haystack);
          const needleStr = String(needle);
          let start = Number(init);
          if (start < 0) start = Math.max(0, hay.length + start);
          // Pattern matching is not supported; only plain substring search.
          void plain;
          return hay.indexOf(needleStr, start);
        };
      }
      if (name === 'extern_is') {
        return externIs;
      }
      if (name === 'getElement') {
        return () => ({ value: 42 });
      }
      if (name === 'Element.value') {
        return (element, delta) => element.value + delta;
      }
      if (name === 'dom_document') {
        return () => domHost.document;
      }
      if (name === 'dom_window') {
        return () => domHost.window;
      }
      if (name === 'dom_create_element') {
        return (_document, tagName) => domHost.createElement(String(tagName));
      }
      if (name === 'dom_set_text') {
        return (element, text) => setText(element, text, name);
      }
      if (name === 'dom_append_child') {
        return (parent, child) => appendChild(parent, child, name);
      }
      if (name === 'Document.create_element') {
        return (_document, tagName) => domHost.createElement(String(tagName));
      }
      if (name === 'Document.get_body' || name === 'Document.get_document_element') {
        return (document) => documentBody(document, name);
      }
      if (name === 'Document.set_body' || name === 'Document.set_document_element') {
        return () => {
          throw new Error(`${name} is read-only`);
        };
      }
      if (name === 'Document.get_element_by_id') {
        return (document, id) => getElementById(asElement(document, name), String(id));
      }
      if (name === 'Document.query_selector') {
        return (document, selectors) => asElement(documentBody(document, name), name).querySelector(String(selectors));
      }
      if (name === 'Window.get_document') {
        return () => domHost.document;
      }
      if (name === 'Window.set_document') {
        return () => {
          throw new Error('Window.document is read-only');
        };
      }
      if (name === 'Window.get_local_storage') {
        return () => domHost.storage;
      }
      if (name === 'Window.set_local_storage') {
        return () => {
          throw new Error('Window.local_storage is read-only');
        };
      }
      if (name === 'Document.append_child' || name === 'Element.append_child' || name === 'Node.append_child') {
        return (parent, child) => appendChild(parent, child, name);
      }
      if (name === 'Node.replace_child') {
        return (parent, newChild, oldChild) => {
          const removed = replaceChild(parent, newChild, oldChild, name);
          cleanupEventListeners(removed);
          return removed;
        };
      }
      if (name === 'Node.remove_child') {
        return (parent, child) => {
          const removed = removeChild(parent, child, name);
          cleanupEventListeners(removed);
          return removed;
        };
      }
      if (name === 'Element.clear') {
        return (element) => {
          const target = asElement(element, name);
          cleanupEventListeners(target);
          target.replaceChildren();
        };
      }
      if (name === 'Event.get_target') {
        return (event) => asEvent(event, name).target;
      }
      if (name === 'Event.set_target') {
        return () => {
          throw new Error('Event.target is read-only');
        };
      }
      if (name === 'EventTarget.add_event_listener') {
        return (target, type, callback) => registerEventListener(target, type, callback, name);
      }
      if (name === 'Element.on_click') {
        return (element, callback) => registerEventListener(element, 'click', callback, name);
      }
      if (name === 'Element.on_input') {
        return (element, callback) => registerEventListener(element, 'input', callback, name);
      }
      if (name === 'Element.set_text') {
        return (element, text) => setText(element, text, name);
      }
      if (name === 'Node.get_node_name') {
        return (node) => asNode(node, name).nodeName ?? asNode(node, name).tagName ?? '';
      }
      if (name === 'Node.get_text_content') {
        return (node) => asNode(node, name).textContent ?? '';
      }
      if (name === 'Node.set_text_content') {
        return (node, text) => {
          asNode(node, name).textContent = String(text);
        };
      }
      if (name === 'Element.get_id') {
        return (element) => asElement(element, name).id ?? '';
      }
      if (name === 'Element.set_id') {
        return (element, id) => {
          asElement(element, name).id = String(id);
        };
      }
      if (name === 'Element.get_class_name') {
        return (element) => asElement(element, name).className ?? '';
      }
      if (name === 'Element.set_class_name') {
        return (element, className) => {
          asElement(element, name).className = String(className);
        };
      }
      if (name === 'Element.append_class') {
        return (element, className) => appendClass(element, className, name);
      }
      if (name === 'Element.get_attribute') {
        return (element, attrName) => asElement(element, name).getAttribute(validateAttrName(attrName));
      }
      if (name === 'Element.set_attribute' || name === 'Element.set_attr') {
        return (element, attrName, value) => {
          const target = asElement(element, name);
          const normalized = validateAttrName(attrName);
          const attrValue = String(value);
          target.setAttribute(normalized, attrValue);
          if (normalized === 'value' && 'value' in target) {
            target.value = attrValue;
          }
        };
      }
      if (name === 'Element.remove_attribute') {
        return (element, attrName) => {
          asElement(element, name).removeAttribute(validateAttrName(attrName));
        };
      }
      if (name === 'Element.query_selector') {
        return (element, selectors) => asElement(element, name).querySelector(String(selectors));
      }
      if (name === 'Element.get_inner_text') {
        return (element) => asElement(element, name).textContent;
      }
      if (name === 'Element.set_inner_text') {
        return (element, text) => setText(element, text, name);
      }
      if (name === 'HTMLElement.get_inner_text') {
        return (element) => getInnerText(element, name);
      }
      if (name === 'HTMLElement.set_inner_text') {
        return (element, text) => setInnerText(element, text, name);
      }
      if (name === 'HTMLElement.get_value') {
        return (element) => String(asElement(element, name).value ?? '');
      }
      if (name === 'HTMLElement.set_value') {
        return (element, value) => {
          asElement(element, name).value = String(value);
        };
      }
      if (name === 'Storage.get_item') {
        return (storage, key) => asStorage(storage, name).getItem(String(key));
      }
      if (name === 'Storage.set_item') {
        return (storage, key, value) => {
          asStorage(storage, name).setItem(String(key), String(value));
        };
      }
      if (name === 'Storage.remove_item') {
        return (storage, key) => {
          asStorage(storage, name).removeItem(String(key));
        };
      }
      if (name === 'bytes_literal') {
        return (index) => {
          const literal = bytesConstants[index];
          if (!literal) throw new Error(`Unknown bytes literal index ${index}`);
          return literal.slice();
        };
      }
      if (name === 'bytes_get') {
        return (value, index) => {
          const bytes = asBytes(value);
          if (index < 0 || index >= bytes.length) throw new Error(`bytes index out of bounds: ${index}`);
          return bytes[index];
        };
      }
      if (name === 'bytes_len') {
        return (value) => asBytes(value).length;
      }
      if (name === 'bytes_concat') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          const merged = new Uint8Array(a.length + b.length);
          merged.set(a, 0);
          merged.set(b, a.length);
          return merged;
        };
      }
      if (name === 'bytes_eq') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          if (a.length !== b.length) return 0;
          for (let i = 0; i < a.length; i++) {
            if (a[i] !== b[i]) return 0;
          }
          return 1;
        };
      }
      if (name === 'bytes_compare') {
        return (left, right) => {
          const a = asBytes(left);
          const b = asBytes(right);
          const len = Math.min(a.length, b.length);
          for (let i = 0; i < len; i++) {
            if (a[i] < b[i]) return -1;
            if (a[i] > b[i]) return 1;
          }
          if (a.length < b.length) return -1;
          if (a.length > b.length) return 1;
          return 0;
        };
      }
      return () => {
        throw new Error(`Unsupported waluau import: ${name}`);
      };
    },
  });
  return {
    [WALUAU_IMPORT_MODULE]: waluauImports,
  };
}

export async function compileAndInstantiate(files, entryFile = '/main.walu') {
  await compileAndInstantiateWithExports(files, entryFile);
}

export async function compileAndInstantiateWithExports(files, entryFile = '/main.walu', options = {}) {
  const module = await import('./waluau-wasm/waluau_wasm.js');
  await module.default();
  const output = module.compile_multi(files, entryFile);
  const wasmBuffer = new Uint8Array(output.wasm);
  let instanceExports = null;
  const imports = buildWaluauImports(wasmBuffer, {
    ...options,
    getWasmExports: () => instanceExports,
  });
  const result = await WebAssembly.instantiate(wasmBuffer, imports, {
    builtins: ['js-string'],
    importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
  });
  instanceExports = result.instance.exports;
  return result.instance.exports;
}

export async function compileAndInstantiateWithDom(files, entryFile = '/main.walu') {
  const domHost = createDomHost();
  const exports = await compileAndInstantiateWithExports(files, entryFile, { domHost });
  return {
    exports,
    root: domHost.root,
    storage: domHost.storage,
  };
}
