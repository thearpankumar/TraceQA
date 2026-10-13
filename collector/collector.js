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

  var COLLECTOR_VERSION = 1;
  var TEXT_LIMIT = 200;
  var includeHidden = !options || options.includeHidden !== false;

  var INTERACTIVE_ROLES = new Set([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "textbox", "searchbox", "combobox", "listbox", "slider",
    "spinbutton", "treeitem",
  ]);
  // Roles whose accessible name may come from the element's own text.
  var NAME_FROM_CONTENT = new Set([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "treeitem",
  ]);
  var INPUT_ROLES = {
    button: "button", submit: "button", reset: "button", image: "button",
    checkbox: "checkbox", radio: "radio", range: "slider", number: "spinbutton", search: "searchbox",
  };
  var LANDMARK_TAGS = {
    main: "main", nav: "navigation", aside: "complementary", search: "search",
    ul: "list", ol: "list", menu: "list", li: "listitem",
    table: "table", tr: "row", td: "cell", th: "columnheader",
    article: "article", dialog: "dialog", fieldset: "group", details: "group",
  };
  var SKIPPED_TAGS = new Set(["script", "style", "template", "noscript", "head", "option", "optgroup"]);
  // Their content belongs to the element itself or to another frame.
  var OPAQUE_TAGS = new Set(["select", "textarea", "svg", "iframe", "frame", "object", "embed"]);
  var COPIED_ATTRS = ["id", "data-testid", "type", "href", "name", "placeholder"];

  var roleCache = new WeakMap();
  var elements = [];
  var warnings = [];

  function squash(text) {
    return (text || "").replace(/\s+/g, " ").trim().slice(0, TEXT_LIMIT);
  }

  function attr(el, name) {
    return (el.getAttribute(name) || "").trim();
  }

  // ---- roles ---------------------------------------------------------------

  function isEditableHost(el) {
    return el.hasAttribute("contenteditable") && attr(el, "contenteditable").toLowerCase() !== "false";
  }

  function hasLabel(el) {
    return attr(el, "aria-label") !== "" || attr(el, "aria-labelledby") !== "";
  }

  function insideSection(el) {
    return !!(el.parentElement && el.parentElement.closest("article, aside, main, nav, section"));
  }

  function implicitRole(el) {
    var tag = el.localName;
    if (tag === "a" || tag === "area") return el.hasAttribute("href") ? "link" : "generic";
    if (tag === "button" || tag === "summary") return "button";
    if (tag === "select") return el.multiple || el.size > 1 ? "listbox" : "combobox";
    if (tag === "textarea") return "textbox";
    if (tag === "input") {
      var type = attr(el, "type").toLowerCase() || "text";
      if (type === "hidden") return "generic";
      if (INPUT_ROLES[type]) return INPUT_ROLES[type];
      return el.hasAttribute("list") ? "combobox" : "textbox";
    }
    if (tag === "header") return insideSection(el) ? "generic" : "banner";
    if (tag === "footer") return insideSection(el) ? "generic" : "contentinfo";
    if (tag === "section") return hasLabel(el) ? "region" : "generic";
    if (tag === "form") return hasLabel(el) ? "form" : "generic";
    if (LANDMARK_TAGS[tag]) return LANDMARK_TAGS[tag];
    return isEditableHost(el) ? "textbox" : "generic";
  }

  function roleOf(el) {
    var cached = roleCache.get(el);
    if (cached) return cached;
    var explicit = attr(el, "role").split(/\s+/)[0].toLowerCase();
    var role = explicit === "presentation" || explicit === "none" ? "generic" : explicit || implicitRole(el);
    roleCache.set(el, role);
    return role;
  }

  // ---- names ---------------------------------------------------------------

  // Text inside an element, including alt text. Leaves out form controls and anything marked
  // aria-hidden. It does not check CSS, so text hidden by a stylesheet is still included.
  function contentText(node) {
    var out = "";
    node.childNodes.forEach(function (child) {
      if (child.nodeType === 3) {
        out += child.nodeValue;
      } else if (child.nodeType === 1) {
        var tag = child.localName;
        if (SKIPPED_TAGS.has(tag) || attr(child, "aria-hidden") === "true") return;
        if (tag === "img") out += " " + attr(child, "alt") + " ";
        else if (tag === "svg") out += " " + (attr(child, "aria-label") || squash((child.querySelector("title") || {}).textContent)) + " ";
        else if (tag !== "input" && tag !== "select" && tag !== "textarea") out += " " + contentText(child) + " ";
      }
    });
    return out;
  }

  function labelledByText(el) {
    var ids = attr(el, "aria-labelledby");
    if (!ids) return "";
    var root = el.getRootNode();
    return squash(ids.split(/\s+/).map(function (id) {
      var target = root.getElementById ? root.getElementById(id) : null;
      return target ? contentText(target) : "";
    }).join(" "));
  }

  // Returns [name, source]. A simplified version of the accessible-name algorithm.
  function nameOf(el, role, fromContent) {
    var tag = el.localName;
    var name = labelledByText(el);
    if (name) return [name, "aria-labelledby"];
    name = squash(attr(el, "aria-label"));
    if (name) return [name, "aria-label"];
    if (el.labels && el.labels.length) {
      name = squash(Array.prototype.map.call(el.labels, contentText).join(" "));
      if (name) return [name, "label"];
    }
    if (tag === "input" && role === "button") {
      name = squash(attr(el, "type").toLowerCase() === "image" ? attr(el, "alt") : attr(el, "value"));
      if (name) return [name, attr(el, "type").toLowerCase() === "image" ? "alt" : "value"];
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
    var first = el.firstElementChild;
    if (first && ((el.localName === "fieldset" && first.localName === "legend") ||
                  (el.localName === "table" && first.localName === "caption"))) {
      return squash(contentText(first));
    }
    return "";
  }

  // ---- selectors -----------------------------------------------------------

  function quote(value) {
    return '"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"';
  }

  // Ids made by frameworks change between builds or page loads.
  function stableId(id) {
    return id !== "" && !/\d{4,}|[0-9a-f]{8,}|^[:_]|^\d/.test(id);
  }

  function matchesOnce(root, selector) {
    try {
      return root.querySelectorAll(selector).length === 1;
    } catch (e) {
      return false;
    }
  }

  function structuralSelector(el, root) {
    var parts = [];
    for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
      var id = attr(node, "id");
      if (node !== el && stableId(id) && matchesOnce(root, "#" + CSS.escape(id))) {
        parts.unshift("#" + CSS.escape(id));
        break;
      }
      var tag = CSS.escape(node.localName);
      if (tag === "body" || tag === "html") {
        parts.unshift(tag);
        break;
      }
      var position = 1;
      var twins = 0;
      var siblings = (node.parentNode || {}).children || [];
      for (var i = 0; i < siblings.length; i++) {
        if (siblings[i].localName !== node.localName) continue;
        twins++;
        if (siblings[i] === node) position = twins;
      }
      parts.unshift(twins > 1 ? tag + ":nth-of-type(" + position + ")" : tag);
    }
    // A path that ran up to a shadow root has nothing to hold on to, so "div > button" would
    // also match deeper in the tree. ":host >" pins it to the top of the shadow root.
    if (root.host && parts[0].charAt(0) !== "#") parts.unshift(":host");
    return parts.join(" > ");
  }

  // Selectors within the element's own tree (the document or one shadow root), most stable first.
  function localSelectors(el) {
    var root = el.getRootNode();
    var found = [];
    var testId = attr(el, "data-testid");
    if (testId && matchesOnce(root, "[data-testid=" + quote(testId) + "]")) found.push("[data-testid=" + quote(testId) + "]");
    var id = attr(el, "id");
    if (stableId(id) && matchesOnce(root, "#" + CSS.escape(id))) found.push("#" + CSS.escape(id));
    found.push(structuralSelector(el, root));
    return found;
  }

  // An element inside shadow DOM is addressed as "host >>> inner": each part is
  // resolved inside the shadow root of the element the previous part matched.
  function selectorsOf(el) {
    var prefix = "";
    for (var root = el.getRootNode(); root && root.host; root = root.host.getRootNode()) {
      prefix = localSelectors(root.host)[0] + " >>> " + prefix;
    }
    return localSelectors(el).map(function (selector) { return prefix + selector; });
  }

  // ---- interactivity -------------------------------------------------------

  function isNative(el) {
    var tag = el.localName;
    if (tag === "button" || tag === "select" || tag === "textarea" || tag === "summary") return true;
    if (tag === "a" || tag === "area") return el.hasAttribute("href");
    return tag === "input" && attr(el, "type").toLowerCase() !== "hidden";
  }

  // Returns { confidence, signals } or null. `ownPointer` is true when this element sets a
  // pointer cursor itself and did not inherit it.
  function evidenceOf(el, ownPointer) {
    var signals = [];
    if (isNative(el)) signals.push("native");
    if (INTERACTIVE_ROLES.has(attr(el, "role").split(/\s+/)[0].toLowerCase())) signals.push("role");
    if (isEditableHost(el)) signals.push("contenteditable");
    var confidence = signals.length ? "high" : "low";
    if (signals.indexOf("native") < 0 && el.hasAttribute("tabindex") && el.tabIndex >= 0) signals.push("tabindex");
    if (el.hasAttribute("onclick")) signals.push("onclick-attr");
    // A label passes its click to its control, which is listed on its own.
    if (confidence === "low" && ownPointer && el.localName !== "label") signals.push("cursor-pointer");
    return signals.length ? { confidence: confidence, signals: signals } : null;
  }

  function round(n) {
    return Math.round(n * 10) / 10;
  }

  function ariaFlag(el, name) {
    var value = attr(el, name).toLowerCase();
    return value === "true" ? true : value === "false" ? false : undefined;
  }

  function stateOf(el, rect, style) {
    var visible = rect.width > 0 && rect.height > 0 && style.visibility === "visible";
    var state = {
      visible: visible,
      enabled: !el.matches(":disabled") && ariaFlag(el, "aria-disabled") !== true,
      inViewport: visible && rect.bottom > 0 && rect.right > 0 && rect.top < window.innerHeight && rect.left < window.innerWidth,
    };
    var inputType = el.localName === "input" ? attr(el, "type").toLowerCase() : "";
    var checked = inputType === "checkbox" || inputType === "radio" ? el.checked : ariaFlag(el, "aria-checked");
    var expanded = el.localName === "summary" && el.parentElement && el.parentElement.localName === "details"
      ? el.parentElement.open
      : ariaFlag(el, "aria-expanded");
    if (checked !== undefined) state.checked = checked;
    if (expanded !== undefined) state.expanded = expanded;
    if (ariaFlag(el, "aria-selected") !== undefined) state.selected = ariaFlag(el, "aria-selected");
    if (ariaFlag(el, "aria-pressed") !== undefined) state.pressed = ariaFlag(el, "aria-pressed");
    if (el.readOnly === true || ariaFlag(el, "aria-readonly") === true) state.readonly = true;
    if (el.required === true || ariaFlag(el, "aria-required") === true) state.required = true;
    return state;
  }

  function record(el, role, style, evidence, path, inShadowDom) {
    var rect = el.getBoundingClientRect();
    var state = stateOf(el, rect, style);
    if (!state.visible && !includeHidden) return;

    var named = nameOf(el, role, NAME_FROM_CONTENT.has(role) || evidence.confidence === "low");
    var selectors = selectorsOf(el);
    var attrs = {};
    COPIED_ATTRS.forEach(function (name) {
      if (el.hasAttribute(name)) attrs[name] = el.getAttribute(name).slice(0, 500);
    });

    var item = {
      role: role,
      name: named[0],
      tag: el.localName,
      attrs: attrs,
      selectors: selectors,
      box: { x: round(rect.left + window.scrollX), y: round(rect.top + window.scrollY), w: round(rect.width), h: round(rect.height) },
      state: state,
      inShadowDom: inShadowDom,
      path: path,
      interactive: evidence,
    };
    if (named[1]) item.nameSource = named[1];
    var text = OPAQUE_TAGS.has(el.localName) || el.localName === "input" ? "" : squash(contentText(el));
    if (text) item.text = text;
    elements.push(item);

    if (!named[0]) {
      warnings.push({
        code: "unnamed_interactive_element",
        message: "<" + el.localName + "> at " + selectors[0] + " has no accessible name",
      });
    }
  }

  // ---- walk ----------------------------------------------------------------

  // `parent` is an element or a shadow root. Its children are visited, not `parent` itself.
  function walk(parent, path, insideInteractive, inShadowDom, parentPointer) {
    var children = parent.children;
    var counts = {};
    var keys = [];
    for (var i = 0; i < children.length; i++) {
      var key = children[i].localName + "|" + roleOf(children[i]);
      keys.push(key);
      counts[key] = (counts[key] || 0) + 1;
    }
    var seen = {};

    for (var j = 0; j < children.length; j++) {
      var el = children[j];
      var tag = el.localName;
      var index = seen[keys[j]] || 0;
      seen[keys[j]] = index + 1;
      if (SKIPPED_TAGS.has(tag)) continue;

      var role = roleOf(el);
      var style = getComputedStyle(el);
      var pointer = style.cursor === "pointer";
      var evidence = evidenceOf(el, pointer && !parentPointer);
      var listed = evidence && !(evidence.confidence === "low" && insideInteractive);
      if (listed) record(el, role, style, evidence, path, inShadowDom);

      if (OPAQUE_TAGS.has(tag)) continue;
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
        childPath = path.concat([step]);
      }
      var inside = insideInteractive || !!listed;
      if (el.shadowRoot) walk(el.shadowRoot, childPath, inside, true, pointer);
      walk(el, childPath, inside, inShadowDom, pointer);
    }
  }

  if (document.body) walk(document.body, [], false, false, false);

  return {
    collectorVersion: COLLECTOR_VERSION,
    url: location.href,
    title: document.title,
    scroll: { y: round(window.scrollY), height: document.documentElement.scrollHeight, viewportHeight: window.innerHeight },
    elements: elements,
    warnings: warnings,
  };
})
