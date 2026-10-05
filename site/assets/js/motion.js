// Scroll scenes for the landing page. Without GSAP, or with reduced motion,
// the page stays the static layout the CSS describes.
(() => {
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];

  // Glass bar once the hero has scrolled away (with or without motion).
  const hero = $(".hero");
  const mini = $(".mini");
  new IntersectionObserver(
    ([e]) => {
      const on = !e.isIntersecting;
      mini.classList.toggle("on", on);
      mini.setAttribute("aria-hidden", String(!on));
      $$("a", mini).forEach((a) => (a.tabIndex = on ? 0 : -1));
    },
    { rootMargin: "-60px 0px 0px 0px" },
  ).observe(hero);

  if (matchMedia("(prefers-reduced-motion: reduce)").matches || !window.gsap || !window.ScrollTrigger) return;
  gsap.registerPlugin(ScrollTrigger);
  if (window.MotionPathPlugin) gsap.registerPlugin(MotionPathPlugin);
  document.documentElement.classList.add("motion");

  /* ---------- 1. Hero: the window rises out of the water ---------- */
  const copy = $(".hero-copy");
  const stage = $(".stage");
  const win = $(".window");
  const placeStage = () => {
    // Its visible top edge should sit just below the copy and always peek into
    // the first screen. Tilted back (rotateX 30, scale .84) that edge appears
    // about 14% of the window's height lower than the box itself.
    const tilt = win.offsetHeight * 0.14;
    const edge = Math.min(copy.offsetTop + copy.offsetHeight + 36, innerHeight * 0.84);
    stage.style.setProperty("--stage-top", edge - tilt + "px");
  };
  placeStage();
  ScrollTrigger.addEventListener("refreshInit", placeStage);

  // Scale that makes the window fill the screen below the nav, and the lift that centres it there.
  const NAV = 84;
  const fit = () => Math.min((innerWidth * 0.94) / win.offsetWidth, ((innerHeight - NAV) * 0.88) / win.offsetHeight);
  const lift = () => NAV + (innerHeight - NAV) / 2 - (stage.offsetTop + win.offsetHeight / 2);

  gsap.set(win, { transformOrigin: "50% 50%" });
  gsap
    .timeline({
      scrollTrigger: { trigger: hero, start: "top top", end: "+=140%", scrub: 0.8, pin: true, anticipatePin: 1, invalidateOnRefresh: true },
    })
    .fromTo(win, { rotateX: 30, scale: 0.84 }, { rotateX: 0, scale: fit, ease: "power1.inOut", duration: 1 }, 0)
    .to(stage, { y: lift, ease: "power1.inOut", duration: 1 }, 0)
    .to(copy, { y: -160, opacity: 0, ease: "power1.in", duration: 0.5 }, 0)
    .to(".hero .water, .hero .ripples", { opacity: 0, duration: 0.5 }, 0.15)
    .to(".hero .stars", { y: -80, ease: "none", duration: 1 }, 0)
    .to({}, { duration: 0.3 }); // hold the full-screen window for a beat

  /* ---------- 2. Statement: words light up as you read ---------- */
  const splitWords = (el) => {
    for (const node of [...el.childNodes]) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        splitWords(node);
      } else if (node.nodeType === Node.TEXT_NODE) {
        const frag = document.createDocumentFragment();
        for (const part of node.textContent.split(/(\s+)/)) {
          if (!part) continue;
          if (/^\s+$/.test(part)) frag.append(part);
          else frag.append(Object.assign(document.createElement("span"), { className: "w", textContent: part }));
        }
        node.replaceWith(frag);
      }
    }
  };
  $$("[data-words]").forEach(splitWords);
  gsap.to(".statement .w", {
    opacity: 1,
    stagger: 0.05,
    ease: "none",
    scrollTrigger: { trigger: ".statement", start: "top 78%", end: "bottom 40%", scrub: true },
  });

  /* ---------- 3. Feature tour: one window, five chapters ---------- */
  const txts = $$(".ch-txt");
  const bars = $$(".tour-progress b");
  const n = txts.length;

  gsap.from(".tour-device", {
    y: 180,
    rotateX: 24,
    opacity: 0,
    transformPerspective: 1600,
    ease: "none",
    scrollTrigger: { trigger: ".tour", start: "top bottom", end: "top top", scrub: true },
  });

  // Scrolling only chooses the chapter; the change itself is a fixed-length
  // animation that always plays out in full. While one runs, no other starts;
  // when it ends we go to wherever the reader is by then, in one move. The
  // wipe slides the incoming frame in while its picture slides the other way,
  // and pushes the outgoing picture back under a shade: transforms and opacity
  // only, so it stays on the GPU.
  const masks = $$(".ch-img");
  const pics = masks.map((m) => $("img", m));
  const shades = masks.map((m) => $(".shade", m));
  const DURATION = 0.9;
  let current = 0;
  let wanted = 0;
  let busy = false;
  let layer = 1;

  gsap.set(masks.slice(1), { xPercent: 100 });
  gsap.set(pics.slice(1), { xPercent: -100 });
  gsap.set(bars[0], { scaleX: 1 });

  const show = (to) => {
    const from = current;
    const dir = to > from ? 1 : -1;
    busy = true;
    current = to;
    gsap.set(masks[to], { xPercent: 100 * dir, zIndex: ++layer });
    gsap.set(pics[to], { xPercent: -100 * dir, scale: 1.06 });
    gsap.set(shades[to], { opacity: 0 });
    gsap
      .timeline({
        defaults: { duration: DURATION, ease: "power3.inOut" },
        onComplete: () => {
          // Park the old chapter off to the side, ready for its next entrance.
          gsap.set(masks[from], { xPercent: 100 });
          gsap.set(pics[from], { xPercent: 0, scale: 1 });
          gsap.set(shades[from], { opacity: 0 });
          busy = false;
          wanted = Math.round(pinned.progress * (n - 1));
          if (wanted !== current) show(wanted);
        },
      })
      .to(masks[to], { xPercent: 0 }, 0)
      .to(pics[to], { xPercent: 0, scale: 1 }, 0)
      .to(pics[from], { xPercent: -16 * dir, scale: 0.97 }, 0)
      .to(shades[from], { opacity: 0.6 }, 0)
      .to(txts[from], { y: -32 * dir, autoAlpha: 0, duration: 0.3, ease: "power2.in" }, 0)
      .fromTo(txts[to], { y: 32 * dir, autoAlpha: 0 }, { y: 0, autoAlpha: 1, duration: 0.5, ease: "power2.out" }, DURATION * 0.4)
      .to(bars, { scaleX: (j) => (j <= to ? 1 : 0), duration: 0.6, ease: "power2.out", stagger: 0.04 }, 0);
  };

  const pinned = ScrollTrigger.create({
    trigger: ".tour-pin",
    start: "top top",
    end: () => "+=" + (n - 1) * 75 + "%",
    pin: true,
    anticipatePin: 1,
    snap: { snapTo: 1 / (n - 1), duration: { min: 0.2, max: 0.5 }, delay: 0.1, ease: "power1.inOut" },
    onUpdate: (self) => {
      if (busy) {
        // The gate: while a chapter is still coming in, the page can't run
        // more than half a chapter past it, so a hard flick can't skip the
        // tour. Before the first and after the last chapter it stays open.
        const span = (self.end - self.start) / (n - 1);
        const y = self.scroll();
        const lo = current === 0 ? -Infinity : self.start + (current - 0.5) * span + 1;
        const hi = current === n - 1 ? Infinity : self.start + (current + 0.5) * span - 1;
        if (y > hi) self.scroll(hi);
        else if (y < lo) self.scroll(lo);
        return;
      }
      wanted = Math.round(self.progress * (n - 1));
      if (wanted !== current) show(wanted);
    },
  });

  // A gentle tilt that follows the scroll, so the section still feels alive.
  gsap.fromTo(
    ".device",
    { rotateY: -6, rotateX: 4, transformPerspective: 1800 },
    { rotateY: 6, rotateX: -1, ease: "none", scrollTrigger: { trigger: ".tour-pin", start: "top top", end: () => "+=" + (n - 1) * 75 + "%", scrub: 1 } },
  );

  /* ---------- 4. Import: bookmarks sail into Kade and moor ---------- */
  const harbour = gsap
    .timeline({ scrollTrigger: { trigger: ".switch", start: "top 75%", end: "bottom 35%", scrub: 0.8 } })
    .from(".dock .src", { x: -90, opacity: 0, stagger: 0.15, ease: "power2.out" })
    .fromTo(".dock .rope", { clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0% 0 0)", ease: "none" }, "-=0.2")
    .from(".berth", { scale: 0.4, opacity: 0, rotate: -12, ease: "back.out(1.7)" }, "-=0.15");
  if (window.MotionPathPlugin) {
    const route = (boat, end) => ({ path: boat.dataset.route, align: boat.dataset.route, alignOrigin: [0.5, 0.82], autoRotate: true, start: 0, end });
    const base = harbour.duration() - 0.3;
    $$(".dock .boat").forEach((boat, i) => {
      gsap.set(boat, { motionPath: route(boat, 0), opacity: 0 });
      const at = base + i * 0.35;
      const wake = $(".wake", boat);
      harbour
        .to(boat, { opacity: 1, duration: 0.15 }, at)
        .to(boat, { motionPath: route(boat, 0.885), duration: 1.4, ease: "power2.inOut" }, at)
        .to(wake, { opacity: 1, duration: 0.2 }, at)
        .to(wake, { opacity: 0, duration: 0.3 }, at + 1.1);
    });
  }

  /* ---------- 5. Headings, rows and cards rise into place ---------- */
  const reveal = $$(
    ".sec-head > div, .sec-head > p, .grid4 .cell, .switch > div:first-child, .honest > div:first-child, .ledger .r, .install > .eyebrow, .install h2, .install .sub, .term, .fr",
  );
  reveal.forEach((el) => el.classList.add("reveal"));
  ScrollTrigger.batch(reveal, {
    start: "top 88%",
    once: true,
    onEnter: (batch) => gsap.to(batch, { opacity: 1, y: 0, stagger: 0.08, duration: 0.9, ease: "power3.out", overwrite: true }),
  });

  /* ---------- 6. Install commands type themselves ---------- */
  for (const pre of $$(".term pre")) {
    const cmd = $(".c", pre);
    const done = $(".done", pre);
    const full = cmd.textContent;
    pre.style.minHeight = pre.offsetHeight + "px";
    cmd.textContent = "";
    gsap.set(done, { opacity: 0 });
    ScrollTrigger.create({
      trigger: pre,
      start: "top 85%",
      once: true,
      onEnter: () => {
        let i = 0;
        const tick = setInterval(() => {
          i = Math.min(full.length, i + 2);
          cmd.textContent = full.slice(0, i);
          if (i === full.length) {
            clearInterval(tick);
            gsap.to(done, { opacity: 1, duration: 0.4, delay: 0.25 });
          }
        }, 14);
      },
    });
  }

  /* ---------- 7. The name rises from the harbour ---------- */
  gsap.fromTo(
    ".giant",
    { yPercent: 50, opacity: 0.3 },
    { yPercent: 0, opacity: 1, ease: "none", scrollTrigger: { trigger: "footer", start: "top bottom", end: "bottom bottom", scrub: true } },
  );

  addEventListener("load", () => ScrollTrigger.refresh());
})();
