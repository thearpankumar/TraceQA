// Buggy Shop behaviour. Plain script, no build step.

const PRODUCTS = [
  "Blue Shirt", "Red Hat", "Green Socks", "Black Jeans", "White Sneakers", "Grey Hoodie",
  "Yellow Raincoat", "Brown Belt", "Pink Scarf", "Navy Blazer", "Orange Beanie", "Purple Gloves",
];
const OUT_OF_STOCK = "Yellow Raincoat";

const grid = document.getElementById("grid");
for (const name of PRODUCTS) {
  const item = document.createElement("li");
  const link = document.createElement("a");
  link.href = "/#product-" + name.toLowerCase().replace(/ /g, "-");
  link.textContent = name;
  const add = document.createElement("button");
  add.type = "button";
  add.textContent = "Add to cart";
  add.disabled = name === OUT_OF_STOCK;
  item.append(link, add);
  grid.append(item);
}

// Clickable div: the handler is added in code, so the page markup gives no hint.
document.getElementById("promo").addEventListener("click", () => {
  location.hash = "deals";
});

// Modal that does not exist until the button is clicked.
document.getElementById("size-guide-btn").addEventListener("click", () => {
  const dialog = document.createElement("dialog");
  const close = document.createElement("button");
  close.type = "button";
  close.textContent = "Close size guide";
  close.addEventListener("click", () => dialog.remove());
  dialog.append("Sizes run small. ", close);
  document.body.append(dialog);
  dialog.showModal();
});

document.getElementById("menu-btn").addEventListener("click", (event) => {
  const nav = document.getElementById("main-nav");
  const open = nav.style.display !== "block";
  nav.style.display = open ? "block" : "";
  event.currentTarget.setAttribute("aria-expanded", String(open));
});

customElements.define("quantity-stepper", class extends HTMLElement {
  constructor() {
    super();
    const root = this.attachShadow({ mode: "open" });
    root.innerHTML = `
      <button type="button" aria-label="Decrease quantity">-</button>
      <input type="number" aria-label="Quantity" value="1" min="1" style="width: 4em">
      <button type="button" aria-label="Increase quantity">+</button>`;
    const input = root.querySelector("input");
    const [down, up] = root.querySelectorAll("button");
    down.addEventListener("click", () => { input.value = Math.max(1, Number(input.value) - 1); });
    up.addEventListener("click", () => { input.value = Number(input.value) + 1; });
  }
});

// The "third party" is the same machine on the next port, which is a different origin.
document.querySelector('iframe[name="pay"]').src =
  `${location.protocol}//${location.hostname}:${Number(location.port) + 1}/pay.html`;

// Reviews: 3 on load, 3 more each time the end of the list scrolls into view, 9 in total.
const reviews = document.getElementById("reviews");
const sentinel = document.getElementById("reviews-sentinel");
const REVIEW_LIMIT = 9;

function addReviews() {
  for (let i = 0; i < 3; i++) {
    const item = document.createElement("li");
    const helpful = document.createElement("button");
    helpful.type = "button";
    helpful.textContent = "Helpful";
    item.append(`Review ${reviews.children.length + 1}: it broke, as promised. `, helpful);
    reviews.append(item);
  }
}

addReviews();
const observer = new IntersectionObserver((entries) => {
  if (!entries.some((entry) => entry.isIntersecting)) return;
  // Simulates a network round trip.
  setTimeout(() => {
    addReviews();
    observer.unobserve(sentinel);
    if (reviews.children.length < REVIEW_LIMIT) observer.observe(sentinel);
  }, 150);
});
observer.observe(sentinel);
