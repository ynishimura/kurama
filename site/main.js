// The landing page's motion: the night sky, parallax, the agent demo, the flow
// diagram and the small interactions. Everything stops or settles under
// prefers-reduced-motion, and nothing runs while it is off screen.
(() => {
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const whenVisible = (element, threshold, callback) =>
    new IntersectionObserver(([entry]) => callback(entry.isIntersecting), { threshold }).observe(element);

  /* Copy buttons: the command without prompts and comments */
  for (const button of document.querySelectorAll("button.copy")) {
    button.addEventListener("click", async () => {
      const text = document.getElementById(button.dataset.copy).textContent
        .replace(/^\$ /gm, "")
        .replace(/\s+# .*$/gm, "")
        .trim();
      await navigator.clipboard.writeText(text);
      button.textContent = "Copied";
      button.classList.add("done");
      setTimeout(() => { button.textContent = "Copy"; button.classList.remove("done"); }, 1500);
    });
  }

  /* Top bar turns solid once the page moves */
  const top = document.getElementById("top");
  const markScrolled = () => top.classList.toggle("scrolled", scrollY > 40);
  addEventListener("scroll", markScrolled, { passive: true });
  markScrolled();

  /* Reveal on scroll */
  const revealer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add("in");
      revealer.unobserve(entry.target);
    }
  }, { threshold: 0.15 });
  document.querySelectorAll(".reveal").forEach((element) => revealer.observe(element));

  /* Marquee: a second copy of the chips makes the loop seamless */
  const track = document.getElementById("track");
  for (const chip of [...track.children]) {
    const copy = chip.cloneNode(true);
    copy.setAttribute("aria-hidden", "true");
    track.appendChild(copy);
  }

  /* Starfield with the odd shooting star */
  const hero = document.getElementById("hero");
  const canvas = document.getElementById("sky");
  const ctx = canvas.getContext("2d");
  let stars = [];
  let meteors = [];
  let width = 0;
  let height = 0;
  let heroVisible = true;
  function sizeSky() {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    width = hero.clientWidth;
    height = hero.clientHeight;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    stars = Array.from({ length: Math.round((width * height) / 5200) }, () => ({
      x: Math.random() * width,
      y: Math.random() * height * 0.75,
      r: Math.random() * 1.3 + 0.2,
      phase: Math.random() * Math.PI * 2,
      speed: 0.4 + Math.random() * 1.6,
    }));
  }
  let last = 0;
  let nextMeteor = 2500;
  function drawSky(now) {
    requestAnimationFrame(drawSky);
    const dt = Math.min(now - last, 50);
    last = now;
    if (!heroVisible || document.hidden) return;
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = "#eaf4fc";
    for (const star of stars) {
      ctx.globalAlpha = reduced ? 0.8 : 0.55 + 0.45 * Math.sin(star.phase + now * 0.001 * star.speed);
      ctx.beginPath();
      ctx.arc(star.x, star.y, star.r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (reduced) return;
    nextMeteor -= dt;
    if (nextMeteor <= 0) {
      meteors.push({
        x: width * (0.2 + Math.random() * 0.7),
        y: Math.random() * height * 0.25,
        vx: -(6 + Math.random() * 4),
        vy: 2.5 + Math.random() * 2,
        life: 1,
      });
      nextMeteor = 3500 + Math.random() * 5000;
    }
    for (const m of meteors) {
      m.x += (m.vx * dt) / 16;
      m.y += (m.vy * dt) / 16;
      m.life -= dt / 1400;
      const tailX = m.x - m.vx * 14;
      const tailY = m.y - m.vy * 14;
      const gradient = ctx.createLinearGradient(m.x, m.y, tailX, tailY);
      gradient.addColorStop(0, `rgba(234, 244, 252, ${Math.max(m.life, 0)})`);
      gradient.addColorStop(1, "rgba(234, 244, 252, 0)");
      ctx.strokeStyle = gradient;
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      ctx.moveTo(m.x, m.y);
      ctx.lineTo(tailX, tailY);
      ctx.stroke();
    }
    meteors = meteors.filter((m) => m.life > 0);
  }
  sizeSky();
  addEventListener("resize", sizeSky);
  whenVisible(hero, 0, (visible) => { heroVisible = visible; });
  requestAnimationFrame(drawSky);

  /* Parallax: the moon and the ridges follow the scroll and the pointer */
  if (!reduced) {
    const layers = [...hero.querySelectorAll("[data-depth]")];
    let px = 0, py = 0, tx = 0, ty = 0;
    hero.addEventListener("pointermove", (event) => {
      tx = (event.clientX / innerWidth - 0.5) * 2;
      ty = (event.clientY / innerHeight - 0.5) * 2;
    });
    (function moveLayers() {
      requestAnimationFrame(moveLayers);
      if (!heroVisible) return;
      px += (tx - px) * 0.06;
      py += (ty - py) * 0.06;
      const scrolled = Math.min(scrollY, innerHeight);
      for (const layer of layers) {
        const depth = parseFloat(layer.dataset.depth);
        layer.style.transform = `translate3d(${-px * depth * 40}px, ${scrolled * depth * 0.6 - py * depth * 14}px, 0)`;
      }
    })();
  }

  /* Agent demo: a request in plain words, the command typed, its output */
  const scenes = [
    {
      ask: "List my open GitHub issues.",
      say: "Three are open. The token went into the request, never into this conversation.",
      lines: [
        ["cmd", "kurama api github issues/list-for-authenticated-user -P state=open --jq '.[].title'"],
        ["out", "\"Fix the cursor on the last page\""],
        ["out", "\"Add a Discovery Document loader\""],
        ["out", "\"Document the [agent] policy\""],
      ],
    },
    {
      ask: "How many orders came in last month?",
      say: "48,213, read straight from the Parquet file on S3 with the ops role.",
      lines: [
        ["cmd", "kurama data 's3://my-bucket/orders/2026-08.parquet' --aws-profile ops --query 'SELECT count(*) FROM data' --jq '.rows[0][0]'"],
        ["out", "\"48213\""],
      ],
    },
    {
      ask: "Which buckets does prod have?",
      say: "Two, listed with a read-only session. The role credentials lived only in that one command's environment.",
      lines: [
        ["cmd", "kurama exec prod --readonly -- aws s3 ls"],
        ["dim", "# Running in readonly mode"],
        ["out", "2026-03-02 10:14:07 app-logs-prod"],
        ["out", "2026-05-19 08:41:52 orders-archive-prod"],
      ],
    },
    {
      ask: "Open an issue for the flaky test.",
      say: "The policy stopped me before any credential was read. May I run it with --confirm?",
      lines: [
        ["cmd", "kurama api github \"POST /repos/o/r/issues\" -d '{\"title\":\"Flaky test\"}'"],
        ["err", "error[AGENT_POLICY_DENIED]: the [agent] policy refuses POST /repos/o/r/issues: POST is not in allow_methods"],
        ["hint", "hint: ask a person whether this call may be made, then rerun it with --confirm; to allow it for every agent run, widen [agent] or [api.<name>.agent] in config.toml"],
      ],
    },
  ];
  const screen = document.getElementById("screen");
  const askText = document.querySelector("#ask span");
  const sayBubble = document.getElementById("say");
  const sayText = sayBubble.querySelector("span");
  const dotBar = document.getElementById("dots");
  const escape = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;");
  const lineClass = { out: "t-out", dim: "t-dim", err: "t-err", hint: "t-hint" };
  const prompt = '<span class="t-p">$ </span>';
  const caret = '<span class="caret"></span>';
  const dots = scenes.map((_, index) => {
    const dot = document.createElement("button");
    dot.type = "button";
    dot.setAttribute("aria-label", `Scene ${index + 1}`);
    dot.addEventListener("click", () => play(index));
    dotBar.appendChild(dot);
    return dot;
  });
  let current = 0;
  let demoVisible = false;
  let started = false;

  async function play(index) {
    const run = ++current;
    const stale = () => run !== current;
    const scene = scenes[index];
    dots.forEach((dot, i) => {
      dot.classList.toggle("on", i === index);
      dot.classList.toggle("done", i < index);
      dot.style.setProperty("--p", 0);
    });
    askText.textContent = scene.ask;
    sayBubble.classList.add("hidden");
    let html = "";
    const show = (tail = "") => { screen.innerHTML = html + tail; };
    const output = (kind, text) => `<span class="${lineClass[kind]}">${escape(text)}</span>\n`;

    if (reduced) {
      for (const [kind, text] of scene.lines) {
        html += kind === "cmd" ? `${prompt}<span class="t-cmd">${escape(text)}</span>\n` : output(kind, text);
      }
      show();
      sayText.textContent = scene.say;
      sayBubble.classList.remove("hidden");
      dots[index].style.setProperty("--p", 1);
      return;
    }

    const typed = scene.lines.reduce((n, [kind, text]) => n + (kind === "cmd" ? text.length : 0), 0);
    const total = 700 + typed * 18 + scene.lines.length * 500 + 4200;
    const began = performance.now();
    (function progress() {
      if (stale()) return;
      dots[index].style.setProperty("--p", Math.min((performance.now() - began) / total, 1));
      requestAnimationFrame(progress);
    })();

    show(prompt + caret);
    await sleep(700);
    for (const [kind, text] of scene.lines) {
      if (stale()) return;
      if (kind === "cmd") {
        html += prompt;
        const step = text.length > 90 ? 2 : 1;
        for (let c = step; c < text.length + step; c += step) {
          if (stale()) return;
          show(`<span class="t-cmd">${escape(text.slice(0, c))}</span>${caret}`);
          await sleep(26);
        }
        html += `<span class="t-cmd">${escape(text)}</span>\n`;
        show(caret);
        await sleep(650);
      } else {
        html += output(kind, text);
        show();
        await sleep(280);
      }
    }
    if (stale()) return;
    show(prompt + caret);
    await sleep(450);
    sayText.textContent = scene.say;
    sayBubble.classList.remove("hidden");
    await sleep(Math.max(total - (performance.now() - began), 2500));
    while (!demoVisible || document.hidden) {
      await sleep(400);
      if (stale()) return;
    }
    if (!stale()) play((index + 1) % scenes.length);
  }
  whenVisible(document.querySelector(".demo"), 0.3, (visible) => {
    demoVisible = visible;
    if (visible && !started) {
      started = true;
      play(0);
    }
  });

  /* Flow diagram: a command out, a secret in, a service call, a result back */
  const particles = document.getElementById("particles");
  const flow = document.querySelector(".flow");
  let flowVisible = false;
  whenVisible(flow, 0.2, (visible) => { flowVisible = visible; });
  function send(pathId, color, duration, delay) {
    setTimeout(() => {
      if (!flowVisible || document.hidden) return;
      const path = document.getElementById(pathId);
      const length = path.getTotalLength();
      const dot = document.createElementNS("http://www.w3.org/2000/svg", "circle");
      dot.setAttribute("r", "5");
      dot.setAttribute("fill", color);
      dot.setAttribute("filter", "url(#glow)");
      particles.appendChild(dot);
      const start = performance.now();
      (function move(now) {
        const k = Math.min((now - start) / duration, 1);
        const eased = k < 0.5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
        const point = path.getPointAtLength(eased * length);
        dot.setAttribute("cx", point.x);
        dot.setAttribute("cy", point.y);
        dot.setAttribute("opacity", k > 0.85 ? (1 - k) / 0.15 : 1);
        if (k < 1) requestAnimationFrame(move);
        else dot.remove();
      })(start);
    }, delay);
  }
  const services = ["w-aws", "w-api", "w-s3", "w-db"];
  function exchange() {
    send("w-cmd", "#38a1db", 1400, 0);
    send("w-vault", "#f5b83d", 1100, 1000);
    const service = services[Math.floor(Math.random() * services.length)];
    send(service, "#38a1db", 1200, 1800);
    setTimeout(() => {
      if (!flowVisible) return;
      const path = document.getElementById(service);
      path.style.stroke = "rgba(127, 198, 106, 0.8)";
      setTimeout(() => { path.style.stroke = ""; }, 600);
    }, 3000);
    send("w-res", "#7fc66a", 1400, 3300);
  }
  if (!reduced) {
    exchange();
    setInterval(exchange, 4600);
  }

  /* Cards: a spotlight under the pointer and a slight tilt */
  for (const card of document.querySelectorAll(".card")) {
    card.addEventListener("pointermove", (event) => {
      const box = card.getBoundingClientRect();
      const x = (event.clientX - box.left) / box.width;
      const y = (event.clientY - box.top) / box.height;
      card.style.setProperty("--mx", `${x * 100}%`);
      card.style.setProperty("--my", `${y * 100}%`);
      if (reduced) return;
      card.classList.add("tilting");
      card.style.setProperty("--ry", `${(x - 0.5) * 6}deg`);
      card.style.setProperty("--rx", `${(0.5 - y) * 6}deg`);
    });
    card.addEventListener("pointerleave", () => {
      card.classList.remove("tilting");
      card.style.setProperty("--rx", "0deg");
      card.style.setProperty("--ry", "0deg");
    });
  }

  /* Showcase tabs cross-fade between the recordings */
  const stageImage = document.getElementById("stage-img");
  const stageCaption = document.getElementById("stage-cap");
  const tabs = [...document.querySelectorAll(".tabs button")];
  for (const tab of tabs) {
    tab.addEventListener("click", () => {
      if (tab.getAttribute("aria-selected") === "true") return;
      tabs.forEach((other) => other.setAttribute("aria-selected", String(other === tab)));
      stageImage.classList.add("swap");
      setTimeout(() => {
        stageImage.onload = () => stageImage.classList.remove("swap");
        stageImage.src = tab.dataset.src;
        stageCaption.textContent = tab.dataset.cap;
      }, reduced ? 0 : 300);
    });
  }
})();
