/* The behaviour of the demo shop pages. Each part runs only where its elements exist. */
(function () {
  function byId(id) { return document.getElementById(id); }

  function toggle(buttonId, targetId) {
    var b = byId(buttonId), t = byId(targetId);
    if (!b || !t) return;
    b.addEventListener("click", function () {
      var open = b.getAttribute("aria-expanded") === "true";
      b.setAttribute("aria-expanded", open ? "false" : "true");
      t.hidden = open;
    });
  }
  toggle("help-btn", "help");
  toggle("more-btn", "more");
  if (byId("help-close")) byId("help-close").addEventListener("click", function () { byId("help-btn").click(); });

  // The promo link's text changes on every load when the server rotates it, as a banner does.
  var promo = byId("promo");
  if (promo) {
    fetch("/shop/api/promo").then(function (r) { return r.text(); }).then(function (text) { promo.textContent = text; });
  }

  [1, 2].forEach(function (n) {
    var tab = byId("tab-" + n);
    if (!tab) return;
    tab.addEventListener("click", function () {
      [1, 2].forEach(function (m) {
        byId("tab-" + m).setAttribute("aria-selected", String(m === n));
        byId("panel-" + m).hidden = m !== n;
      });
    });
  });

  // The offer is made once for as long as the server runs: the first person to ask gets it, the next gets nothing.
  var offerBtn = byId("offer-btn");
  if (offerBtn) {
    offerBtn.addEventListener("click", function () {
      fetch("/shop/api/offer").then(function (r) { return r.json(); }).then(function (data) {
        if (!data.offer) return;
        byId("offer-text").textContent = data.offer;
        byId("offer").hidden = false;
        offerBtn.setAttribute("aria-expanded", "true");
      });
    });
    byId("offer-close").addEventListener("click", function () {
      byId("offer").hidden = true;
      offerBtn.setAttribute("aria-expanded", "false");
    });
  }

  function send(buttonId, method, path) {
    var b = byId(buttonId);
    if (b) b.addEventListener("click", function () { fetch(path, { method: method, body: method === "DELETE" ? undefined : "x" }); });
  }
  send("add", "POST", "/shop/api/cart");
  send("clear", "POST", "/shop/api/clear");
  send("delete", "DELETE", "/shop/api/account");

  // What is typed is reported as a request, so that a test can read it from the server's log.
  document.querySelectorAll("input").forEach(function (input) {
    input.addEventListener("input", function () {
      var label = input.closest("label").firstChild.textContent.trim();
      fetch("/shop/api/typed/" + encodeURIComponent(label) + "/" + encodeURIComponent(input.value), { method: "POST", body: "x" });
    });
  });
})();
