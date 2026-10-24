(function traceqaPristine() {
  "use strict";
  // Captures the browser's own built-in functions, so that what a page does to its
  // prototypes afterwards cannot change what the collector reads. The result is a table
  // of "uncurried" functions: table.getAttribute(element, "id") calls the original
  // Element.prototype.getAttribute without looking it up on the page-modifiable prototype.
  //
  // This text exists in three places: this file, the capture block inside collector.js (used
  // when the page already ran its scripts), and the Playwright driver (which runs it before the
  // page's scripts, as an init script). THIS FILE IS THE ONLY ONE TO EDIT. Afterwards run
  // `python3 scripts/sync_pristine.py` to rewrite the other two; CI and a test run it with
  // --check. Do not use a triple double quote or a backslash in this file: the driver holds the
  // text in a raw Python string.
  //
  // Best effort: a page that ran before this did can have replaced anything, and
  // Function.prototype.toString, which tells native code from page code, can be faked too.
  var uncurry = Function.prototype.bind.bind(Function.prototype.call);
  var describe = Object.getOwnPropertyDescriptor;
  var parentOf = Object.getPrototypeOf;
  var fnToString = uncurry(Function.prototype.toString);
  var strIndexOf = uncurry(String.prototype.indexOf);

  var T = {};
  var watched = [];

  function find(owner, key) {
    try {
      for (var obj = owner; obj; obj = parentOf(obj)) {
        var d = describe(obj, key);
        if (d) return d;
      }
    } catch (e) { /* a hostile getter or proxy: treat as missing */ }
    return null;
  }

  function proto(name) {
    var ctor = typeof self !== "undefined" ? self[name] : undefined;
    return ctor && ctor.prototype ? ctor.prototype : null;
  }

  function watch(label, owner, key, kind) {
    var d = owner ? find(owner, key) : null;
    var fn = d ? (kind === "get" ? d.get : d.value) : null;
    if (typeof fn !== "function") fn = null;
    watched[watched.length] = { label: label, owner: owner, key: key, kind: kind, original: fn };
    return fn;
  }

  function method(name, label, owner, key) {
    var fn = watch(label, owner, key, "value");
    T[name] = fn ? uncurry(fn) : null;
  }

  function getter(name, label, owner, key) {
    var fn = watch(label, owner, key, "get");
    T[name] = fn ? uncurry(fn) : null;
  }

  // Functions with no receiver to speak of: Math.round(x), CSS.escape(x).
  function staticFn(name, label, owner, key, arity) {
    var fn = watch(label, owner, key, "value");
    if (!fn) {
      T[name] = null;
      return;
    }
    var call = uncurry(fn);
    T[name] = arity === 2
      ? function (a, b) { return call(owner, a, b); }
      : function (a) { return call(owner, a); };
  }

  // Only to notice a change: the collector does not call these.
  function watchOnly(label, owner, key) {
    watch(label, owner, key, "value");
  }

  var ArrayP = Array.prototype;
  var StringP = String.prototype;
  var NodeP = proto("Node");
  var ElementP = proto("Element");
  var DocumentP = proto("Document");
  var FragmentP = proto("DocumentFragment");
  var ShadowP = proto("ShadowRoot");
  var NodeListP = proto("NodeList");
  var RectP = proto("DOMRectReadOnly");

  method("push", "Array.prototype.push", ArrayP, "push");
  method("unshift", "Array.prototype.unshift", ArrayP, "unshift");
  method("concat", "Array.prototype.concat", ArrayP, "concat");
  method("indexOf", "Array.prototype.indexOf", ArrayP, "indexOf");
  method("join", "Array.prototype.join", ArrayP, "join");
  method("sliceArray", "Array.prototype.slice", ArrayP, "slice");
  method("map", "Array.prototype.map", ArrayP, "map");
  method("trim", "String.prototype.trim", StringP, "trim");
  method("sliceString", "String.prototype.slice", StringP, "slice");
  method("toLowerCase", "String.prototype.toLowerCase", StringP, "toLowerCase");
  method("charCodeAt", "String.prototype.charCodeAt", StringP, "charCodeAt");
  method("stringIndexOf", "String.prototype.indexOf", StringP, "indexOf");
  method("weakMapGet", "WeakMap.prototype.get", WeakMap.prototype, "get");
  method("weakMapSet", "WeakMap.prototype.set", WeakMap.prototype, "set");
  watchOnly("Set.prototype.has", Set.prototype, "has");
  watchOnly("Set.prototype.add", Set.prototype, "add");
  watchOnly("Function.prototype.toString", Function.prototype, "toString");

  method("getAttribute", "Element.prototype.getAttribute", ElementP, "getAttribute");
  method("hasAttribute", "Element.prototype.hasAttribute", ElementP, "hasAttribute");
  method("matches", "Element.prototype.matches", ElementP, "matches");
  method("closest", "Element.prototype.closest", ElementP, "closest");
  method("elementQuerySelector", "Element.prototype.querySelector", ElementP, "querySelector");
  method("elementQuerySelectorAll", "Element.prototype.querySelectorAll", ElementP, "querySelectorAll");
  method("getRootNode", "Node.prototype.getRootNode", NodeP, "getRootNode");
  method("getBoundingClientRect", "Element.prototype.getBoundingClientRect", ElementP, "getBoundingClientRect");
  method("checkVisibility", "Element.prototype.checkVisibility", ElementP, "checkVisibility");
  method("contains", "Node.prototype.contains", NodeP, "contains");
  method("documentElementFromPoint", "Document.prototype.elementFromPoint", DocumentP, "elementFromPoint");
  method("documentQuerySelectorAll", "Document.prototype.querySelectorAll", DocumentP, "querySelectorAll");
  method("documentGetElementById", "Document.prototype.getElementById", DocumentP, "getElementById");
  method("rootElementFromPoint", "ShadowRoot.prototype.elementFromPoint", ShadowP, "elementFromPoint");
  method("fragmentQuerySelectorAll", "DocumentFragment.prototype.querySelectorAll", FragmentP, "querySelectorAll");
  method("fragmentGetElementById", "DocumentFragment.prototype.getElementById", FragmentP, "getElementById");
  method("getPropertyValue", "CSSStyleDeclaration.prototype.getPropertyValue", proto("CSSStyleDeclaration"), "getPropertyValue");

  staticFn("round", "Math.round", Math, "round", 1);
  staticFn("max", "Math.max", Math, "max", 2);
  staticFn("min", "Math.min", Math, "min", 2);
  staticFn("cssEscape", "CSS.escape", typeof CSS !== "undefined" ? CSS : null, "escape", 1);
  staticFn("getComputedStyle", "window.getComputedStyle", typeof window !== "undefined" ? window : null, "getComputedStyle", 1);

  getter("nodeType", "Node.prototype.nodeType", NodeP, "nodeType");
  getter("nodeValue", "Node.prototype.nodeValue", NodeP, "nodeValue");
  getter("firstChild", "Node.prototype.firstChild", NodeP, "firstChild");
  getter("nextSibling", "Node.prototype.nextSibling", NodeP, "nextSibling");
  getter("parentNode", "Node.prototype.parentNode", NodeP, "parentNode");
  getter("parentElement", "Node.prototype.parentElement", NodeP, "parentElement");
  getter("textContent", "Node.prototype.textContent", NodeP, "textContent");
  getter("localName", "Element.prototype.localName", ElementP, "localName");
  getter("shadowRoot", "Element.prototype.shadowRoot", ElementP, "shadowRoot");
  getter("elementFirstElementChild", "Element.prototype.firstElementChild", ElementP, "firstElementChild");
  getter("fragmentFirstElementChild", "DocumentFragment.prototype.firstElementChild", FragmentP, "firstElementChild");
  getter("documentFirstElementChild", "Document.prototype.firstElementChild", DocumentP, "firstElementChild");
  getter("nextElementSibling", "Element.prototype.nextElementSibling", ElementP, "nextElementSibling");
  getter("host", "ShadowRoot.prototype.host", ShadowP, "host");
  getter("listLength", "NodeList.prototype.length", NodeListP, "length");
  getter("rectLeft", "DOMRect.prototype.left", RectP, "left");
  getter("rectRight", "DOMRect.prototype.right", RectP, "right");
  getter("rectTop", "DOMRect.prototype.top", RectP, "top");
  getter("rectBottom", "DOMRect.prototype.bottom", RectP, "bottom");
  getter("rectWidth", "DOMRect.prototype.width", RectP, "width");
  getter("rectHeight", "DOMRect.prototype.height", RectP, "height");

  // `labels` lives on each kind of form control, and each one's getter only takes its own kind.
  var labelGetters = {};
  var labelKinds = [
    ["input", "HTMLInputElement"], ["select", "HTMLSelectElement"], ["textarea", "HTMLTextAreaElement"],
    ["button", "HTMLButtonElement"], ["meter", "HTMLMeterElement"], ["output", "HTMLOutputElement"],
    ["progress", "HTMLProgressElement"],
  ];
  for (var k = 0; k < labelKinds.length; k++) {
    var labelGet = watch(labelKinds[k][1] + ".prototype.labels", proto(labelKinds[k][1]), "labels", "get");
    if (labelGet) labelGetters[labelKinds[k][0]] = uncurry(labelGet);
  }

  var weakMapNew = WeakMap;
  T.newWeakMap = function () { return new weakMapNew(); };

  // ---- helpers built on the table; they use nothing else ----------------------

  T.labels = function (el) {
    var get = labelGetters[T.localName(el)];
    return get ? get(el) : undefined;
  };

  T.childElements = function (parent) {
    var out = [];
    var type = T.nodeType(parent);
    var first = type === 11 ? T.fragmentFirstElementChild : type === 9 ? T.documentFirstElementChild : T.elementFirstElementChild;
    for (var child = first(parent); child; child = T.nextElementSibling(child)) out[out.length] = child;
    return out;
  };

  T.querySelectorAll = function (root, selector) {
    var type = T.nodeType(root);
    return (type === 9 ? T.documentQuerySelectorAll : type === 11 ? T.fragmentQuerySelectorAll : T.elementQuerySelectorAll)(root, selector);
  };

  T.getElementById = function (root, id) {
    var type = T.nodeType(root);
    if (type === 9) return T.documentGetElementById(root, id);
    if (type === 11) return T.fragmentGetElementById(root, id);
    return null;
  };

  T.hostOf = function (root) {
    if (T.nodeType(root) !== 11) return null;
    try { return T.host(root); } catch (e) { return null; }
  };

  T.rectOf = function (el) {
    var rect = T.getBoundingClientRect(el);
    return {
      left: T.rectLeft(rect), right: T.rectRight(rect), top: T.rectTop(rect), bottom: T.rectBottom(rect),
      width: T.rectWidth(rect), height: T.rectHeight(rect),
    };
  };

  // ---- noticing that a page changed built-ins ---------------------------------

  function looksNative(entry) {
    var fn = entry.original;
    try {
      var text = fnToString(fn);
      var at = strIndexOf(text, "[native code]");
      if (at < 0 || text.length - at > 30) return false;
      // Methods are named for their key. Getters are named differently by each engine.
      return entry.kind === "get" || fn.name === entry.key;
    } catch (e) {
      return false;
    }
  }

  // `replaced`: built-ins whose live version is no longer the one captured here.
  // `suspect`: captured ones that do not look like native code, which is what is left to
  // check when this table was made after the page had already run.
  T.verify = function () {
    var replaced = [];
    var suspect = [];
    for (var i = 0; i < watched.length; i++) {
      var entry = watched[i];
      if (entry.original === null) continue;
      var d = find(entry.owner, entry.key);
      var live = d ? (entry.kind === "get" ? d.get : d.value) : null;
      if (live !== entry.original) replaced[replaced.length] = entry.label;
      if (!looksNative(entry)) suspect[suspect.length] = entry.label;
    }
    return { replaced: replaced, suspect: suspect };
  };

  return Object.freeze(T);
})
