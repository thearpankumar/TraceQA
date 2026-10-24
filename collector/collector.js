// TraceQA collector.
//
// This file is one JavaScript expression that evaluates to a function. A driver
// runs it in every frame of the page and returns what it produces, which is a
// CollectorResult as defined in spec/collector.schema.json.
//
// It reports facts only: which elements can be interacted with, what they are
// called, where they are and what state they are in. It decides nothing about
// grouping or ids. It must run unchanged in every browser, so it uses no
// browser-specific API and has no build step.
(function traceqaCollect(options) {
  "use strict";

  // A page's scripts can replace methods on the built-in prototypes, and the collector would
  // then read the page through the page's functions. So every built-in the collector uses
  // comes from a table of the browser's own originals, N, and nothing below calls a method on
  // a page object directly (the test in collector/tests keeps it that way). Where a driver can
  // run a script before the page's (the Playwright driver does), the table was made then.
  // Otherwise it is made here, and a replaced function is noticed by not looking like native
  // code. Either way the result carries a collector_tampered warning.
  // This is best effort: Function.prototype.toString can be faked too, and what a page does to
  // properties that are not captured (such as the checked state of an input) is not defended.
  // BEGIN pristine.js
  var capture = (function traceqaPristine() {
  "use strict";
  // Captures the browser's own built-in functions, so that what a page does to its
  // prototypes afterwards cannot change what the collector reads. The result is a table
  // of "uncurried" functions: table.getAttribute(element, "id") calls the original
  // Element.prototype.getAttribute without looking it up on the page-modifiable prototype.
  //
  // This text exists in three places that a test keeps identical: this file, the capture
  // block inside collector.js (used when the page already ran its scripts), and the
  // Playwright driver (which runs it before the page's scripts, as an init script).
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
});
  // END pristine.js

  var supplied = null;
  try {
    var offered = window.__traceqa_pristine__;
    if (typeof offered === "function") supplied = offered();
  } catch (e) { /* no table was left by a driver */ }
  if (!supplied || typeof supplied.verify !== "function" || typeof supplied.getAttribute !== "function") supplied = null;
  var local = null;
  try {
    local = capture();
  } catch (e) {
    // The page broke what the capture needs. Without a table from the driver there is nothing to read with.
    if (!supplied) throw e;
  }
  var N = supplied || local;

  var COLLECTOR_VERSION = 1;
  var TEXT_LIMIT = 200;
  var includeHidden = !options || options.includeHidden !== false;

  // Lookup tables have no prototype, so a page that adds to Object.prototype changes nothing.
  function setOf(items) {
    var set = { __proto__: null };
    for (var i = 0; i < items.length; i++) set[items[i]] = true;
    return set;
  }

  var INTERACTIVE_ROLES = setOf([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "textbox", "searchbox", "combobox", "listbox", "slider",
    "spinbutton", "treeitem",
  ]);
  // Roles whose accessible name may come from the element's own text.
  var NAME_FROM_CONTENT = setOf([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "treeitem",
  ]);
  var INPUT_ROLES = {
    __proto__: null,
    button: "button", submit: "button", reset: "button", image: "button",
    checkbox: "checkbox", radio: "radio", range: "slider", number: "spinbutton", search: "searchbox",
  };
  var LANDMARK_TAGS = {
    __proto__: null,
    main: "main", nav: "navigation", aside: "complementary", search: "search",
    ul: "list", ol: "list", menu: "list", li: "listitem",
    table: "table", tr: "row", td: "cell", th: "columnheader",
    article: "article", dialog: "dialog", fieldset: "group", details: "group",
  };
  var SKIPPED_TAGS = setOf(["script", "style", "template", "noscript", "head", "option", "optgroup"]);
  // Their content belongs to the element itself or to another frame.
  var OPAQUE_TAGS = setOf(["select", "textarea", "svg", "iframe", "frame", "object", "embed"]);
  var COPIED_ATTRS = ["id", "data-testid", "type", "href", "name", "placeholder"];

  var roleCache = N.newWeakMap();
  var elements = [];
  var warnings = [];

  // ---- text ----------------------------------------------------------------
  // Whitespace is handled by hand: String.replace and split with a regular expression go
  // through RegExp.prototype, which a page can replace as well.

  function isSpace(code) {
    return (code >= 9 && code <= 13) || code === 32 || code === 160 || code === 0x1680 ||
      (code >= 0x2000 && code <= 0x200a) || code === 0x2028 || code === 0x2029 || code === 0x202f ||
      code === 0x205f || code === 0x3000 || code === 0xfeff;
  }

  function words(text) {
    var out = [];
    var current = "";
    for (var i = 0; i < text.length; i++) {
      if (isSpace(N.charCodeAt(text, i))) {
        if (current !== "") {
          N.push(out, current);
          current = "";
        }
      } else {
        current += text[i];
      }
    }
    if (current !== "") N.push(out, current);
    return out;
  }

  function squash(text) {
    return N.sliceString(N.join(words(text || ""), " "), 0, TEXT_LIMIT);
  }

  function attr(el, name) {
    return N.trim(N.getAttribute(el, name) || "");
  }

  function firstWord(text) {
    return words(text)[0] || "";
  }

  // ---- roles ---------------------------------------------------------------

  function isEditableHost(el) {
    return N.hasAttribute(el, "contenteditable") && N.toLowerCase(attr(el, "contenteditable")) !== "false";
  }

  function hasLabel(el) {
    return attr(el, "aria-label") !== "" || attr(el, "aria-labelledby") !== "";
  }

  function insideSection(el) {
    var parent = N.parentElement(el);
    return !!(parent && N.closest(parent, "article, aside, main, nav, section"));
  }

  function implicitRole(el) {
    var tag = N.localName(el);
    if (tag === "a" || tag === "area") return N.hasAttribute(el, "href") ? "link" : "generic";
    if (tag === "button" || tag === "summary") return "button";
    if (tag === "select") return el.multiple || el.size > 1 ? "listbox" : "combobox";
    if (tag === "textarea") return "textbox";
    if (tag === "input") {
      var type = N.toLowerCase(attr(el, "type")) || "text";
      if (type === "hidden") return "generic";
      if (INPUT_ROLES[type]) return INPUT_ROLES[type];
      return N.hasAttribute(el, "list") ? "combobox" : "textbox";
    }
    if (tag === "header") return insideSection(el) ? "generic" : "banner";
    if (tag === "footer") return insideSection(el) ? "generic" : "contentinfo";
    if (tag === "section") return hasLabel(el) ? "region" : "generic";
    if (tag === "form") return hasLabel(el) ? "form" : "generic";
    if (LANDMARK_TAGS[tag]) return LANDMARK_TAGS[tag];
    return isEditableHost(el) ? "textbox" : "generic";
  }

  // ARIA ignores role="presentation" or "none" on an element that can take focus or that
  // carries its own label: it stays what it is, so a <button role="none"> is still a button.
  function presentationApplies(el) {
    return !(isNative(el) || N.hasAttribute(el, "tabindex") || isEditableHost(el) || hasLabel(el) ||
             N.hasAttribute(el, "aria-describedby"));
  }

  function roleOf(el) {
    var cached = N.weakMapGet(roleCache, el);
    if (cached) return cached;
    var explicit = N.toLowerCase(firstWord(attr(el, "role")));
    var presentational = explicit === "presentation" || explicit === "none";
    var role = presentational ? (presentationApplies(el) ? "generic" : implicitRole(el)) : explicit || implicitRole(el);
    N.weakMapSet(roleCache, el, role);
    return role;
  }

  // ---- names ---------------------------------------------------------------

  // Text inside an element, including alt text. Leaves out form controls and anything marked
  // aria-hidden. It does not check CSS, so text hidden by a stylesheet is still included.
  function contentText(node) {
    var out = "";
    for (var child = N.firstChild(node); child; child = N.nextSibling(child)) {
      var type = N.nodeType(child);
      if (type === 3) {
        out += N.nodeValue(child);
      } else if (type === 1) {
        var tag = N.localName(child);
        if (SKIPPED_TAGS[tag] === true || attr(child, "aria-hidden") === "true") continue;
        if (tag === "img") {
          out += " " + attr(child, "alt") + " ";
        } else if (tag === "svg") {
          var title = N.elementQuerySelector(child, "title");
          out += " " + (attr(child, "aria-label") || squash(title ? N.textContent(title) : "")) + " ";
        } else if (tag !== "input" && tag !== "select" && tag !== "textarea") {
          out += " " + contentText(child) + " ";
        }
      }
    }
    return out;
  }

  function labelledByText(el) {
    var ids = attr(el, "aria-labelledby");
    if (!ids) return "";
    var root = N.getRootNode(el);
    var parts = [];
    var list = words(ids);
    for (var i = 0; i < list.length; i++) {
      var target = N.getElementById(root, list[i]);
      N.push(parts, target ? contentText(target) : "");
    }
    return squash(N.join(parts, " "));
  }

  // Returns [name, source]. A simplified version of the accessible-name algorithm.
  function nameOf(el, role, fromContent) {
    var tag = N.localName(el);
    var name = labelledByText(el);
    if (name) return [name, "aria-labelledby"];
    name = squash(attr(el, "aria-label"));
    if (name) return [name, "aria-label"];
    var labels = N.labels(el);
    if (labels && N.listLength(labels)) {
      var texts = [];
      for (var i = 0; i < N.listLength(labels); i++) N.push(texts, contentText(labels[i]));
      name = squash(N.join(texts, " "));
      if (name) return [name, "label"];
    }
    if (tag === "input" && role === "button") {
      var isImage = N.toLowerCase(attr(el, "type")) === "image";
      name = squash(isImage ? attr(el, "alt") : attr(el, "value"));
      if (name) return [name, isImage ? "alt" : "value"];
    }
    if (fromContent) {
      name = squash(contentText(el));
      if (name) return [name, "content"];
    }
    name = squash(attr(el, "title"));
    if (name) return [name, "title"];
    name = squash(attr(el, "placeholder"));
    if (name) return [name, "placeholder"];
    return ["", ""];
  }

  // Ancestors only get a name when the page states one. Deriving it from content would be slow and unstable.
  function ancestorName(el) {
    var name = labelledByText(el) || squash(attr(el, "aria-label"));
    if (name) return name;
    var first = N.elementFirstElementChild(el);
    var tag = N.localName(el);
    if (first && ((tag === "fieldset" && N.localName(first) === "legend") ||
                  (tag === "table" && N.localName(first) === "caption"))) {
      return squash(contentText(first));
    }
    return "";
  }

  // ---- selectors -----------------------------------------------------------

  function quote(value) {
    var out = '"';
    for (var i = 0; i < value.length; i++) {
      var ch = value[i];
      out += ch === "\\" ? "\\\\" : ch === '"' ? '\\"' : ch;
    }
    return out + '"';
  }

  function isDigit(code) {
    return code >= 48 && code <= 57;
  }

  function isHex(code) {
    return isDigit(code) || (code >= 97 && code <= 102);
  }

  // Ids made by frameworks change between builds or page loads: four digits in a row, eight
  // hex digits in a row, or a start with a digit, colon or underscore.
  function stableId(id) {
    if (id === "") return false;
    var first = N.charCodeAt(id, 0);
    if (isDigit(first) || first === 58 || first === 95) return false;
    var digits = 0;
    var hex = 0;
    for (var i = 0; i < id.length; i++) {
      var code = N.charCodeAt(id, i);
      digits = isDigit(code) ? digits + 1 : 0;
      hex = isHex(code) ? hex + 1 : 0;
      if (digits >= 4 || hex >= 8) return false;
    }
    return true;
  }

  function matchesOnce(root, selector) {
    try {
      return N.listLength(N.querySelectorAll(root, selector)) === 1;
    } catch (e) {
      return false;
    }
  }

  function structuralSelector(el, root) {
    var parts = [];
    for (var node = el; node && N.nodeType(node) === 1; node = N.parentElement(node)) {
      var id = attr(node, "id");
      if (node !== el && stableId(id) && matchesOnce(root, "#" + N.cssEscape(id))) {
        N.unshift(parts, "#" + N.cssEscape(id));
        break;
      }
      var name = N.localName(node);
      var tag = N.cssEscape(name);
      if (tag === "body" || tag === "html") {
        N.unshift(parts, tag);
        break;
      }
      var position = 1;
      var twins = 0;
      var parent = N.parentNode(node);
      var siblings = parent ? N.childElements(parent) : [];
      for (var i = 0; i < siblings.length; i++) {
        if (N.localName(siblings[i]) !== name) continue;
        twins++;
        if (siblings[i] === node) position = twins;
      }
      N.unshift(parts, twins > 1 ? tag + ":nth-of-type(" + position + ")" : tag);
    }
    // A path that ran up to a shadow root has nothing to hold on to, so "div > button" would
    // also match deeper in the tree. ":host >" pins it to the top of the shadow root.
    if (N.hostOf(root) && parts[0][0] !== "#") N.unshift(parts, ":host");
    return N.join(parts, " > ");
  }

  // Selectors within the element's own tree (the document or one shadow root), most stable first.
  function localSelectors(el) {
    var root = N.getRootNode(el);
    var found = [];
    var testId = attr(el, "data-testid");
    if (testId && matchesOnce(root, "[data-testid=" + quote(testId) + "]")) N.push(found, "[data-testid=" + quote(testId) + "]");
    var id = attr(el, "id");
    if (stableId(id) && matchesOnce(root, "#" + N.cssEscape(id))) N.push(found, "#" + N.cssEscape(id));
    N.push(found, structuralSelector(el, root));
    return found;
  }

  // An element inside shadow DOM is addressed as "host >>> inner": each part is
  // resolved inside the shadow root of the element the previous part matched.
  function selectorsOf(el) {
    var prefix = "";
    for (var root = N.getRootNode(el); root && N.hostOf(root); root = N.getRootNode(N.hostOf(root))) {
      prefix = localSelectors(N.hostOf(root))[0] + " >>> " + prefix;
    }
    var own = localSelectors(el);
    var out = [];
    for (var i = 0; i < own.length; i++) N.push(out, prefix + own[i]);
    return out;
  }

  // ---- interactivity -------------------------------------------------------

  function isNative(el) {
    var tag = N.localName(el);
    if (tag === "button" || tag === "select" || tag === "textarea" || tag === "summary") return true;
    if (tag === "a" || tag === "area") return N.hasAttribute(el, "href");
    return tag === "input" && N.toLowerCase(attr(el, "type")) !== "hidden";
  }

  // Returns { confidence, signals } or null. `ownPointer` is true when this element sets a
  // pointer cursor itself and did not inherit it.
  function evidenceOf(el, ownPointer) {
    var signals = [];
    if (isNative(el)) N.push(signals, "native");
    if (INTERACTIVE_ROLES[N.toLowerCase(firstWord(attr(el, "role")))] === true) N.push(signals, "role");
    if (isEditableHost(el)) N.push(signals, "contenteditable");
    var confidence = signals.length ? "high" : "low";
    if (N.indexOf(signals, "native") < 0 && N.hasAttribute(el, "tabindex") && el.tabIndex >= 0) N.push(signals, "tabindex");
    if (N.hasAttribute(el, "onclick")) N.push(signals, "onclick-attr");
    // A label passes its click to its control, which is listed on its own.
    if (confidence === "low" && ownPointer && N.localName(el) !== "label") N.push(signals, "cursor-pointer");
    return signals.length ? { confidence: confidence, signals: signals } : null;
  }

  function round(n) {
    return N.round(n * 10) / 10;
  }

  function ariaFlag(el, name) {
    var value = N.toLowerCase(attr(el, name));
    return value === "true" ? true : value === "false" ? false : undefined;
  }

  // The numbers in a string such as "rect(0,0,0,0)": runs of digits and dots, with a minus sign.
  function numbersIn(text) {
    var out = [];
    var i = 0;
    while (i < text.length) {
      var start = i;
      var code = N.charCodeAt(text, i);
      if (code === 45) i++;
      var from = i;
      while (i < text.length && (isDigit(N.charCodeAt(text, i)) || N.charCodeAt(text, i) === 46)) i++;
      if (i > from) N.push(out, N.sliceString(text, start, i));
      else i = start + 1;
    }
    return out;
  }

  // The usual ways of hiding something from sight while keeping it for screen readers, as
  // skip links and "visually hidden" text are: one pixel or less, or clipped away entirely.
  function visuallyHidden(rect, style) {
    if (rect.width <= 1 && rect.height <= 1) return true;
    var clip = N.join(words(N.getPropertyValue(style, "clip") || ""), "");
    if (N.sliceString(clip, 0, 5) === "rect(") {
      var edges = numbersIn(clip);
      if (edges.length === 4 && edges[0] === edges[2] && edges[1] === edges[3]) return true;
    }
    var clipPath = N.join(words(N.getPropertyValue(style, "clip-path") || ""), "");
    return clipPath === "inset(50%)" || clipPath === "inset(100%)";
  }

  // True when a click at this point would reach the element: the hit is the element, part of
  // it, one of its labels, or an ancestor (the element itself lets clicks through to it).
  function reaches(el, hit) {
    if (hit === el || N.contains(el, hit) || N.contains(hit, el)) return true;
    var labels = N.labels(el);
    var count = labels ? N.listLength(labels) : 0;
    for (var i = 0; i < count; i++) {
      if (labels[i] === hit || N.contains(labels[i], hit)) return true;
    }
    return false;
  }

  // The element on top of `el` at every point tested, or null when a click can reach it.
  // Only what is on screen can be tested, so the part of the box inside the viewport is used.
  var COVER_POINTS = [[0.5, 0.5], [0.2, 0.2], [0.8, 0.2], [0.2, 0.8], [0.8, 0.8]];
  function coverOf(el, rect) {
    var left = N.max(rect.left, 0), right = N.min(rect.right, window.innerWidth);
    var top = N.max(rect.top, 0), bottom = N.min(rect.bottom, window.innerHeight);
    if (right - left < 1 || bottom - top < 1) return null;
    var cover = null;
    for (var i = 0; i < COVER_POINTS.length; i++) {
      var hit = topmostAt(left + (right - left) * COVER_POINTS[i][0], top + (bottom - top) * COVER_POINTS[i][1]);
      if (!hit || reaches(el, hit)) return null;
      cover = cover || hit;
    }
    return cover;
  }

  // The element on top at a point, looking into open shadow roots. The document answers with
  // the shadow host, and each shadow root then says which of its elements is on top. Asking a
  // shadow root directly is not the same in every browser: Firefox answers null for a point
  // where something outside that shadow root is on top.
  function topmostAt(x, y) {
    var hit = N.documentElementFromPoint(document, x, y);
    while (hit && N.rootElementFromPoint) {
      var shadow = N.shadowRoot(hit);
      if (!shadow) break;
      var inner = N.rootElementFromPoint(shadow, x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    return hit;
  }

  function stateOf(el, rect, style) {
    var visible = rect.width > 0 && rect.height > 0 && N.getPropertyValue(style, "visibility") === "visible";
    // The browser's own answer also covers what size and visibility miss: the content of a
    // closed <details>, other content-visibility: hidden content, and opacity 0. Browsers
    // from before 2024 do not have it and keep the answer above.
    if (visible && N.checkVisibility) {
      visible = N.checkVisibility(el, { checkOpacity: true, checkVisibilityCSS: true });
    }
    if (visible && visuallyHidden(rect, style)) visible = false;
    var state = {
      visible: visible,
      enabled: !N.matches(el, ":disabled") && ariaFlag(el, "aria-disabled") !== true,
      inViewport: visible && rect.bottom > 0 && rect.right > 0 && rect.top < window.innerHeight && rect.left < window.innerWidth,
    };
    var tag = N.localName(el);
    var inputType = tag === "input" ? N.toLowerCase(attr(el, "type")) : "";
    var checked = inputType === "checkbox" || inputType === "radio" ? el.checked : ariaFlag(el, "aria-checked");
    var parent = N.parentElement(el);
    var expanded = tag === "summary" && parent && N.localName(parent) === "details"
      ? parent.open
      : ariaFlag(el, "aria-expanded");
    if (checked !== undefined) state.checked = checked;
    if (expanded !== undefined) state.expanded = expanded;
    if (ariaFlag(el, "aria-selected") !== undefined) state.selected = ariaFlag(el, "aria-selected");
    if (ariaFlag(el, "aria-pressed") !== undefined) state.pressed = ariaFlag(el, "aria-pressed");
    if (el.readOnly === true || ariaFlag(el, "aria-readonly") === true) state.readonly = true;
    if (el.required === true || ariaFlag(el, "aria-required") === true) state.required = true;
    if (state.visible && state.inViewport) {
      var cover = coverOf(el, rect);
      state.covered = !!cover;
      if (cover) state.coveredBy = selectorsOf(cover)[0];
    }
    return state;
  }

  function record(el, role, style, evidence, path, inShadowDom) {
    var rect = N.rectOf(el);
    var state = stateOf(el, rect, style);
    if (!state.visible && !includeHidden) return;

    var named = nameOf(el, role, NAME_FROM_CONTENT[role] === true || evidence.confidence === "low");
    var selectors = selectorsOf(el);
    var attrs = {};
    for (var a = 0; a < COPIED_ATTRS.length; a++) {
      var name = COPIED_ATTRS[a];
      if (N.hasAttribute(el, name)) attrs[name] = N.sliceString(N.getAttribute(el, name), 0, 500);
    }

    var tag = N.localName(el);
    var item = {
      role: role,
      name: named[0],
      tag: tag,
      attrs: attrs,
      selectors: selectors,
      box: { x: round(rect.left + window.scrollX), y: round(rect.top + window.scrollY), w: round(rect.width), h: round(rect.height) },
      state: state,
      inShadowDom: inShadowDom,
      path: path,
      interactive: evidence,
    };
    if (named[1]) item.nameSource = named[1];
    var text = OPAQUE_TAGS[tag] === true || tag === "input" ? "" : squash(contentText(el));
    if (text) item.text = text;
    N.push(elements, item);

    if (!named[0]) {
      N.push(warnings, {
        code: "unnamed_interactive_element",
        message: "<" + tag + "> at " + selectors[0] + " has no accessible name",
      });
    }
  }

  // ---- walk ----------------------------------------------------------------

  // `parent` is an element or a shadow root. Its children are visited, not `parent` itself.
  function walk(parent, path, insideInteractive, inShadowDom, parentPointer) {
    var children = N.childElements(parent);
    var counts = { __proto__: null };
    var keys = [];
    for (var i = 0; i < children.length; i++) {
      var key = N.localName(children[i]) + "|" + roleOf(children[i]);
      N.push(keys, key);
      counts[key] = (counts[key] || 0) + 1;
    }
    var seen = { __proto__: null };

    for (var j = 0; j < children.length; j++) {
      var el = children[j];
      var tag = N.localName(el);
      var index = seen[keys[j]] || 0;
      seen[keys[j]] = index + 1;
      if (SKIPPED_TAGS[tag] === true) continue;

      var role = roleOf(el);
      var style = N.getComputedStyle(el);
      var pointer = N.getPropertyValue(style, "cursor") === "pointer";
      var evidence = evidenceOf(el, pointer && !parentPointer);
      var listed = evidence && !(evidence.confidence === "low" && insideInteractive);
      if (listed) record(el, role, style, evidence, path, inShadowDom);

      if (OPAQUE_TAGS[tag] === true) continue;
      var repeatCount = counts[keys[j]];
      var childPath = path;
      if (role !== "generic" || repeatCount >= 2) {
        var step = { role: role, tag: tag };
        var stepName = ancestorName(el);
        if (stepName) step.name = stepName;
        if (repeatCount >= 2) {
          step.repeatIndex = index;
          step.repeatCount = repeatCount;
        }
        childPath = N.concat(path, [step]);
      }
      var inside = insideInteractive || !!listed;
      var shadow = N.shadowRoot(el);
      if (shadow) walk(shadow, childPath, inside, true, pointer);
      walk(el, childPath, inside, inShadowDom, pointer);
    }
  }

  // ---- tampering -----------------------------------------------------------

  // Names of built-ins the page replaced. With a table from before the page's scripts the
  // live functions are compared with it. Whatever table is in use, the live functions are
  // also checked for looking like native code.
  function tamperedNames() {
    var names = [];
    var lists = [];
    if (supplied) N.push(lists, supplied.verify().replaced);
    if (local) N.push(lists, local.verify().suspect);
    for (var l = 0; l < lists.length; l++) {
      for (var i = 0; i < lists[l].length; i++) {
        if (N.indexOf(names, lists[l][i]) < 0) N.push(names, lists[l][i]);
      }
    }
    return names;
  }

  function tamperWarning(names, stopped) {
    var shown = names.length > 6 ? N.concat(N.sliceArray(names, 0, 6), ["and " + (names.length - 6) + " more"]) : names;
    return {
      code: "collector_tampered",
      message: "The page replaced built-in functions (" + N.join(shown, ", ") + "). " +
        (supplied
          ? "Results were read with the browser's original functions."
          : "Results may be wrong; the browser could not supply the originals.") +
        (stopped ? " Reading stopped early and the result is partial." : ""),
    };
  }

  var tampered = [];
  try {
    tampered = tamperedNames();
  } catch (e) { /* the check itself is best effort */ }

  var stopped = false;
  try {
    if (document.body) walk(document.body, [], false, false, false);
  } catch (error) {
    // Without a sign of tampering this is a fault in the collector: let it surface.
    if (tampered.length === 0) throw error;
    stopped = true;
  }
  if (tampered.length) N.unshift(warnings, tamperWarning(tampered, stopped));

  return {
    collectorVersion: COLLECTOR_VERSION,
    url: location.href,
    title: document.title,
    scroll: { y: round(window.scrollY), height: document.documentElement.scrollHeight, viewportHeight: window.innerHeight },
    elements: elements,
    warnings: warnings,
  };
})
