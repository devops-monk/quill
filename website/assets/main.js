// Screenshot viewer: segmented tabs swap the image and caption. Arrow keys move
// between tabs, as in a native segmented control.
(() => {
  const viewer = document.querySelector("[data-viewer]");
  if (!viewer) return;
  const tabs = [...viewer.querySelectorAll('[role="tab"]')];
  const img = viewer.querySelector("[data-screen-img]");
  const caption = viewer.querySelector("[data-screen-caption]");

  // Warm the cache so switching tabs never shows a blank frame.
  tabs.forEach((tab) => { new Image().src = tab.dataset.src; });

  function select(tab) {
    tabs.forEach((t) => {
      const selected = t === tab;
      t.setAttribute("aria-selected", String(selected));
      t.tabIndex = selected ? 0 : -1;
    });
    img.classList.add("fading");
    setTimeout(() => {
      img.src = tab.dataset.src;
      img.alt = `Quill: ${tab.textContent.toLowerCase()}`;
      caption.textContent = tab.dataset.caption;
      img.classList.remove("fading");
    }, 180);
  }

  tabs.forEach((tab, index) => {
    tab.tabIndex = index === 0 ? 0 : -1;
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (!step) return;
      const next = tabs[(index + step + tabs.length) % tabs.length];
      next.focus();
      select(next);
    });
  });
})();

// Hero demo: type the question, then reveal the tables used, the SQL and the
// answer. Plays once when the window scrolls into view; reduced-motion users
// see the finished state immediately.
(() => {
  const demo = document.querySelector("[data-demo]");
  if (!demo) return;

  const typed = demo.querySelector("[data-typed]");
  const question = typed.textContent;
  const steps = ["tables", "sql", "answer"].map((name) => demo.querySelector(`[data-step="${name}"]`));
  const usedTables = ["products", "order_items", "orders"].map((name) => demo.querySelector(`[data-table="${name}"]`));
  const replay = document.querySelector("[data-replay]");
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  let running = false;

  function showFinal() {
    typed.textContent = question;
    usedTables.forEach((el) => el.classList.add("used"));
    steps.forEach((el) => el.classList.add("shown"));
  }

  function reset() {
    typed.textContent = "";
    usedTables.forEach((el) => el.classList.remove("used"));
    steps.forEach((el) => el.classList.remove("shown"));
  }

  async function play() {
    if (running) return;
    running = true;
    replay.hidden = true;
    reset();
    await wait(400);
    for (const char of question) {
      typed.textContent += char;
      await wait(char === " " ? 55 : 34);
    }
    await wait(450);
    usedTables.forEach((el) => el.classList.add("used"));
    steps[0].classList.add("shown");
    await wait(700);
    steps[1].classList.add("shown");
    await wait(900);
    steps[2].classList.add("shown");
    await wait(1200);
    replay.hidden = false;
    running = false;
  }

  if (reduceMotion || !("IntersectionObserver" in window)) {
    showFinal();
    return;
  }

  reset();
  const observer = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer.disconnect();
        play();
      }
    },
    { threshold: 0.35 },
  );
  observer.observe(demo);
  replay.addEventListener("click", play);
})();
