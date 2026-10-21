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

// Download links: read the latest release from GitHub and point each link at
// its installer. Until a release exists every link opens the releases page.
(() => {
  const section = document.querySelector("[data-download]");
  if (!section) return;
  const status = section.querySelector("[data-release-status]");
  const primary = section.querySelector("[data-primary-download]");
  const patterns = {
    "mac-arm": /_aarch64\.dmg$/,
    "mac-intel": /_x64\.dmg$/,
    "win-x64": /_x64-setup\.exe$/,
    "win-x64-msi": /_x64_en-US\.msi$/,
    "win-arm": /_arm64-setup\.exe$/,
    "linux-appimage": /_amd64\.AppImage$/,
    "linux-deb": /_amd64\.deb$/,
    "linux-rpm": /\.x86_64\.rpm$/,
    "linux-arm-appimage": /_aarch64\.AppImage$/,
    "linux-arm-deb": /_arm64\.deb$/,
  };

  function visitorPlatform() {
    const ua = navigator.userAgent;
    const platform = navigator.userAgentData?.platform ?? navigator.platform ?? "";
    if (/Win/i.test(platform) || /Windows/i.test(ua)) return { key: /ARM|aarch64/i.test(ua) ? "win-arm" : "win-x64", label: "Windows" };
    if (/Mac/i.test(platform) || /Mac OS X/i.test(ua)) return { key: "mac-arm", label: "Mac (Apple silicon)" };
    if (/Linux/i.test(platform) || /Linux/i.test(ua)) return { key: /aarch64|arm/i.test(ua) ? "linux-arm-appimage" : "linux-appimage", label: "Linux" };
    return null;
  }

  fetch("https://api.github.com/repos/devops-monk/quill/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then((response) => (response.ok ? response.json() : Promise.reject(response.status)))
    .then((release) => {
      const assets = release.assets ?? [];
      const urls = {};
      for (const [key, pattern] of Object.entries(patterns)) {
        const asset = assets.find((a) => pattern.test(a.name));
        if (asset) urls[key] = asset.browser_download_url;
      }
      section.querySelectorAll("[data-asset]").forEach((link) => {
        const url = urls[link.dataset.asset];
        if (url) link.href = url;
      });
      status.textContent = `Version ${release.tag_name.replace(/^v/, "")}, free and open source, for macOS, Windows and Linux.`;
      const visitor = visitorPlatform();
      if (visitor && urls[visitor.key]) {
        primary.href = urls[visitor.key];
        primary.textContent = `Download for ${visitor.label}`;
        section.querySelector(`[data-asset="${visitor.key}"]`)?.classList.add("recommended");
      }
    })
    .catch(() => {
      status.textContent = "The first release is on its way. Installers will appear here as soon as it's published.";
    });
})();
