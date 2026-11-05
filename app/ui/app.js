// Voxen's window. Your bots are voices on a stage: the room's cast sits at the top, the
// conversation runs below as a script, and the newest line is large and alive.

const tauri = window.__TAURI__;
const call = tauri
  ? { invoke: tauri.core.invoke, listen: tauri.event.listen }
  : { invoke: () => Promise.reject("Open this in the Voxen app to chat."), listen: () => {} };

// Each voice's hue; everything else stays black and white. Handed out in order to bots without one.
const HUES = [
  { name: "Iris", hex: "#8E7CFF" },
  { name: "Tide", hex: "#2EC4B6" },
  { name: "Ember", hex: "#FF6B3D" },
  { name: "Sol", hex: "#FFC233" },
  { name: "Sky", hex: "#5AB0FF" },
  { name: "Bloom", hex: "#FF5FA2" },
];
const TRAITS = ["Warm", "Witty", "Calm", "Direct", "Curious", "Upbeat", "Patient", "Bold"];
const TEMPLATES = [
  { label: "Researcher", name: "Nova", role: "Finds facts, compares options and checks the numbers", traits: ["Calm", "Direct"], color: "#5AB0FF" },
  { label: "Coach", name: "Rio", role: "Keeps you on track with goals and habits", traits: ["Warm", "Upbeat"], color: "#FFC233" },
  { label: "Planner", name: "Ava", role: "Plans trips, days and events down to the details", traits: ["Upbeat", "Curious"], color: "#FF6B3D" },
  { label: "Writer", name: "Quill", role: "Drafts and polishes messages, posts and emails", traits: ["Witty", "Warm"], color: "#FF5FA2" },
  { label: "Tutor", name: "Theo", role: "Explains anything, step by step", traits: ["Patient", "Warm"], color: "#2EC4B6" },
];
// Offered when your voice list can't be fetched (no ElevenLabs key yet).
const FALLBACK_VOICES = [
  { id: "EXAVITQu4vr4xnSDxMaL", name: "Sarah" },
  { id: "JBFqnCBsd6RMkjVDRZzb", name: "George" },
];
const SUGGESTIONS = {
  main: ["Plan my week", "Explain something simply", "Help me write a message", "Give me a dinner idea"],
  bot: ["What can you do?", "Help me think through a decision", "Teach me something new"],
  group: ["Debate it: Rust or Go?", "Plan a weekend trip together", "Pros and cons of working from home"],
};

const $ = (id) => document.getElementById(id);
const stage = $("stage");
const input = $("input");
const composer = $("composer");
const still = matchMedia("(prefers-reduced-motion: reduce)");

let team = { main: "", bots: [], groups: [] };
let keys = { claude: false, deepgram: false, elevenlabs: false, xai: false };
let voiceReady = false;
let open = "main";
let voiceOn = false;
let mode = "quick"; // quick, think or research
let tray = []; // files waiting to go with your next turn
let pendingOpen = null; // a room to open once its new voice or room shows up
let hearing = null; // what you're saying, before your turn ends
let demo = []; // timers of a running demo
const scripts = {}; // room → its script
const lives = {}; // room → the line being spoken there
const lastSpeaker = {}; // room → who spoke last, for passing the turn

const same = (a, b) => a.trim().toLowerCase() === b.trim().toLowerCase();
const bot = (name) => team.bots.find((b) => same(b.name, name));
const group = (name) => team.groups.find((g) => same(g.name, name));
const taken = (name) => Boolean(bot(name) || group(name));
const and = (names) => (names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names.at(-1)}` : names[0] || "");
const now = () => new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const ink = () => getComputedStyle(document.documentElement).getPropertyValue("--text").trim() || "#f4f4f6";

function hue(name) {
  const b = bot(name);
  if (b?.color) return b.color;
  const i = Math.max(0, team.bots.findIndex((x) => same(x.name, name)));
  return HUES[i % HUES.length].hex;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function button(className, text, onClick) {
  const b = el("button", className, text);
  b.type = "button";
  b.addEventListener("click", onClick);
  return b;
}

// Voiceprints: a ring of bars shaped by a name, so every voice has its own mark. It sits
// still, breathes while listening, sweeps while thinking and moves while speaking.

let ticking = false; // whether moving prints are being drawn

function seeded(text) {
  let h = 2166136261;
  for (const ch of text.toLowerCase()) h = Math.imul(h ^ ch.charCodeAt(0), 16777619);
  return () => {
    h ^= h << 13;
    h ^= h >>> 17;
    h ^= h << 5;
    return (h >>> 0) / 4294967296;
  };
}

function voiceprint(name, color, size, letter = true) {
  const rand = seeded(name || "voice");
  const raw = Array.from({ length: 60 }, () => 0.12 + 0.88 * rand() ** 1.5);
  const bars = raw.map((v, i) => (raw[(i + 59) % 60] + v * 2 + raw[(i + 1) % 60]) / 4); // smooth neighbours into one shape
  const canvas = el("canvas", "print");
  const ratio = window.devicePixelRatio || 1;
  canvas.width = canvas.height = Math.round(size * ratio);
  canvas.style.width = canvas.style.height = `${size}px`;
  canvas.setAttribute("aria-hidden", "true");
  const print = { canvas, name, color, size: size * ratio, bars, letter, state: "idle", seed: rand() * 100 };
  canvas.print = print;
  draw(print, performance.now() / 1000);
  return canvas;
}

function motion(p, i, n, t) {
  const phase = t + p.seed;
  if (p.state === "speaking") return 0.5 + 0.5 * Math.abs(Math.sin(phase * 6.5 + i * 0.85) * Math.sin(phase * 2.9 + i * 0.31));
  if (p.state === "thinking") return 0.32 + 0.68 * Math.max(0, Math.cos(phase * 3.2 - (i / n) * Math.PI * 2)) ** 4;
  if (p.state === "listening") return 0.52 + 0.1 * Math.sin(phase * 1.3 + i * 0.18);
  return 0.5;
}

function draw(p, t) {
  const ctx = p.canvas.getContext("2d");
  const s = p.size;
  const c = s / 2;
  const n = p.bars.length;
  const inner = s * (p.letter ? 0.27 : 0.2);
  const reach = s * (p.letter ? 0.25 : 0.3);
  ctx.clearRect(0, 0, s, s);
  ctx.lineCap = "round";
  ctx.lineWidth = Math.max(1, s * (p.letter ? 0.02 : 0.03));
  ctx.strokeStyle = p.color;
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2 - Math.PI / 2;
    const m = motion(p, i, n, t);
    const len = reach * p.bars[i] * m + s * 0.012;
    ctx.globalAlpha = p.state === "idle" ? 0.85 : 0.3 + 0.7 * Math.min(1, m);
    ctx.beginPath();
    ctx.moveTo(c + Math.cos(a) * inner, c + Math.sin(a) * inner);
    ctx.lineTo(c + Math.cos(a) * (inner + len), c + Math.sin(a) * (inner + len));
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
  if (p.letter) {
    ctx.fillStyle = p.color;
    ctx.font = `500 ${Math.round(s * 0.26)}px "New York", ui-serif, Georgia, serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText((p.name[0] || "?").toUpperCase(), c, c + s * 0.015);
  }
}

function setPrint(canvas, state) {
  const p = canvas?.print;
  if (!p || p.state === state) return;
  p.state = state;
  draw(p, performance.now() / 1000);
  if (state !== "idle" && !ticking) {
    ticking = true;
    requestAnimationFrame(frame);
  }
}

// Draws the moving prints, and stops once none move, so a quiet Voxen uses no power.
function frame(ms) {
  let moving = false;
  for (const canvas of document.querySelectorAll("canvas.print")) {
    if (canvas.print.state === "idle" || still.matches) continue;
    draw(canvas.print, ms / 1000);
    moving = true;
  }
  ticking = moving;
  if (moving) requestAnimationFrame(frame);
}

// Rooms are "main", "bot:<name>" and "group:<name>".

function members(room) {
  if (room.startsWith("bot:")) return [room.slice(4)];
  if (room.startsWith("group:")) return group(room.slice(6))?.bots ?? [];
  return [team.main];
}

function exists(room) {
  if (room.startsWith("bot:")) return Boolean(bot(room.slice(4)));
  if (room.startsWith("group:")) return Boolean(group(room.slice(6)));
  return true;
}

function kind(room) {
  return room.startsWith("group:") ? "group" : room === "main" ? "main" : "bot";
}

// The rail

function roomButton(room, icon, title, sub, count) {
  const b = button("room-btn", "", () => openRoom(room));
  b.dataset.room = room;
  const ico = el("span", "ico");
  ico.append(icon);
  if (count) ico.append(el("span", "count", String(count)));
  const lbl = el("span", "lbl");
  lbl.append(el("b", "", title), el("small", "", sub));
  b.append(ico, lbl);
  b.setAttribute("aria-label", `${title}, ${sub}`);
  const li = el("li");
  li.append(b);
  return li;
}

function renderRail() {
  const items = [];
  const mainBot = bot(team.main);
  if (mainBot) items.push(roomButton("main", voiceprint(mainBot.name, hue(mainBot.name), 42), mainBot.name, "Your main voice"));
  for (const b of team.bots.filter((x) => !same(x.name, team.main))) {
    items.push(roomButton(`bot:${b.name}`, voiceprint(b.name, hue(b.name), 42), b.name, b.role));
  }
  for (const g of team.groups) {
    items.push(roomButton(`group:${g.name}`, voiceprint(g.name, ink(), 42, false), g.name, and(g.bots), g.bots.length));
  }
  items.slice(0, 9).forEach((li, i) => {
    li.firstChild.append(el("kbd", "kbd", `⌘${i + 1}`));
    li.firstChild.setAttribute("aria-keyshortcuts", `Meta+${i + 1}`);
  });
  $("rooms").replaceChildren(...items);
  markOpen();
}

function markOpen() {
  document.querySelectorAll(".room-btn").forEach((r) => {
    if (r.dataset.room === open) {
      r.setAttribute("aria-current", "page");
      r.classList.remove("unread");
    } else r.removeAttribute("aria-current");
  });
}

// A room with something new you haven't seen.
function markUnread(room) {
  document.querySelector(`.room-btn[data-room="${CSS.escape(room)}"]`)?.classList.add("unread");
}

function renderSetup() {
  const missing = [keys.claude, keys.deepgram, keys.elevenlabs].filter((ok) => !ok).length;
  $("setup").classList.toggle("ok", missing === 0);
  $("setup-title").textContent = missing === 0 ? "All set" : keys.claude ? "Add voice" : "Finish setup";
  $("setup-sub").textContent = missing === 0 ? "Chat and voice are on" : keys.claude ? `${missing} key${missing > 1 ? "s" : ""} to go` : "Add your keys";
}

// The cast

function renderCast() {
  const names = members(open);
  const size = names.length === 1 ? 150 : names.length === 2 ? 116 : 96;
  $("cast").replaceChildren(
    ...names.map((name) => {
      const card = el("div", "member");
      card.dataset.name = name;
      card.style.setProperty("--c", hue(name));
      const ring = el("span", "glow");
      ring.append(voiceprint(name, hue(name), size));
      card.append(ring, el("span", "n", name), el("span", "s"));
      return card;
    }),
  );
  castState(null, voiceOn ? "listening" : "idle");
}

// Puts `who` in `state` and everyone else in the room at rest, and says so in the status line.
function castState(who, state) {
  const rest = voiceOn ? "listening" : "idle";
  document.querySelectorAll("#cast .member").forEach((card) => {
    const mine = who && same(card.dataset.name, who);
    const s = mine ? state : rest;
    card.classList.toggle("on", Boolean(mine) && state !== "idle");
    card.dataset.state = s;
    card.querySelector(".s").textContent = s === "idle" ? "" : s;
    setPrint(card.querySelector("canvas"), s);
  });
  $("status").textContent =
    who && state === "thinking" ? `${who} is thinking` : who && state === "speaking" ? `${who} is speaking` : voiceOn ? "Listening" : "Voice is off";
}

// A spark that carries the turn from one voice to the next.
function passTurn(from, to) {
  const cards = [...document.querySelectorAll("#cast .member")];
  const a = cards.find((c) => same(c.dataset.name, from))?.querySelector("canvas");
  const b = cards.find((c) => same(c.dataset.name, to))?.querySelector("canvas");
  if (!a || !b || still.matches) return;
  const base = stage.getBoundingClientRect();
  const ra = a.getBoundingClientRect();
  const rb = b.getBoundingClientRect();
  const baton = $("baton");
  baton.style.setProperty("--x0", `${ra.left + ra.width / 2 - base.left - 5}px`);
  baton.style.setProperty("--x1", `${rb.left + rb.width / 2 - base.left - 5}px`);
  baton.style.setProperty("--y", `${ra.top + ra.height / 2 - base.top - 5}px`);
  baton.style.setProperty("--c", hue(to));
  baton.classList.remove("go");
  void baton.offsetWidth; // restart the animation
  baton.classList.add("go");
}

// The script

function scriptFor(room) {
  if (!scripts[room]) {
    const s = el("div", "script");
    s.setAttribute("role", "log");
    s.setAttribute("aria-live", voiceOn ? "off" : "polite");
    s.setAttribute("aria-relevant", "additions");
    s.hidden = room !== open;
    $("scripts").append(s);
    scripts[room] = s;
  }
  return scripts[room];
}

function refreshQuiet() {
  stage.classList.toggle("quiet", !scriptFor(open).childElementCount);
}

// Runs a change to a script, keeping it scrolled to the end if it already was.
function stick(script, change) {
  const atEnd = script.scrollHeight - script.scrollTop - script.clientHeight < 90;
  change();
  if (atEnd) script.scrollTop = script.scrollHeight;
}

function addLine(room, line) {
  const script = scriptFor(room);
  script.querySelectorAll(".line.now").forEach((l) => l.classList.remove("now"));
  stick(script, () => script.append(line));
  if (room === open) refreshQuiet();
  return line;
}

function whoColumn(name, time) {
  const who = el("div", "who", name);
  const meta = el("span", "meta", time);
  who.append(meta);
  return { who, meta };
}

function yourLine(text, files = []) {
  const line = el("div", "line you now");
  const body = el("div", "body");
  body.append(el("div", "said", text));
  if (files.length) {
    const strip = el("div", "files");
    for (const f of files) {
      if (!f.preview) strip.append(el("span", "chip-file", f.name));
      else strip.append(Object.assign(el("img"), { src: f.preview, alt: f.name }));
    }
    body.append(strip);
  }
  line.append(whoColumn("You", now()).who, body);
  return line;
}

const host = (url) => {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
};

function sourceLink(source, i) {
  const a = el("a", "source");
  a.href = source.url;
  a.title = source.title;
  a.dataset.url = source.url;
  a.append(el("b", "", String(i + 1)), host(source.url));
  return a;
}

function picture(url, prompt) {
  const img = Object.assign(el("img", "picture"), { src: url, alt: prompt, title: "Open full size" });
  img.dataset.url = url;
  return img;
}

function botLine(name) {
  const line = el("div", "line now live");
  line.style.setProperty("--c", hue(name));
  line.setAttribute("aria-busy", "true");
  const { who, meta } = whoColumn(name, now());
  const said = el("div", "said");
  const copy = button("tool", "Copy", async () => {
    await navigator.clipboard?.writeText(said.textContent).catch(() => {});
    copy.textContent = "Copied";
    setTimeout(() => (copy.textContent = "Copy"), 1500);
  });
  const tools = el("div", "tools");
  tools.append(copy);
  // What it did with its tools, then what it said, pictures it made and the pages behind it.
  const steps = el("ul", "steps");
  const media = el("div", "media");
  const sources = el("div", "sources");
  const body = el("div", "body");
  body.append(steps, said, media, sources, tools);
  line.append(who, body);
  return { bot: name, line, said, meta, steps, media, sources };
}

// Appends text a word at a time, each word fading in.
function speak(said, text) {
  for (const part of text.split(/(\s+)/)) {
    if (!part) continue;
    if (/^\s+$/.test(part)) said.append(part);
    else said.append(el("span", "w", part));
  }
}

function note(room, text) {
  if (room === open && stage.classList.contains("quiet")) $("note").textContent = text;
  else addLine(room, el("p", "notice", text)).classList.remove("now");
}

function clearHearing() {
  hearing?.remove();
  hearing = null;
}

// Empties a room's script on screen.
function wipe(room) {
  scriptFor(room).replaceChildren();
  delete lives[room];
  delete lastSpeaker[room];
  if (room !== open) return;
  clearHearing();
  refreshQuiet();
  castState(null, "idle");
}

async function openRoom(room) {
  stopDemo();
  if (room !== open) {
    clearHearing();
    await call.invoke("open", { chat: room }).catch(() => {});
  }
  open = room;
  for (const [id, s] of Object.entries(scripts)) s.hidden = id !== room;
  const script = scriptFor(room);
  script.hidden = false;
  const names = members(room);
  const k = kind(room);
  $("room-title").textContent = k === "group" ? room.slice(6) : names[0] || "";
  $("room-sub").textContent = k === "group" ? `${and(names)} and you` : k === "main" ? `Main voice · ${bot(names[0])?.role ?? ""}` : (bot(names[0])?.role ?? "");
  renderCast();
  const line = $("welcome-line");
  const em = el("em", "", k === "group" ? room.slice(6) : names[0] || "Voxen");
  em.style.setProperty("--c", k === "group" ? "var(--text)" : hue(names[0] || ""));
  line.replaceChildren(...(k === "group" ? ["Ask ", em, " anything. They'll talk it over."] : ["Say something to ", em, "."]));
  $("suggestions").replaceChildren(...SUGGESTIONS[k].map((s) => button("suggestion", s, () => send(s))));
  $("note").textContent = "";
  input.placeholder = k === "group" ? "Talk, or type to the room…" : `Talk, or type to ${names[0] || "Voxen"}…`;
  markOpen();
  refreshQuiet();
  script.scrollTop = script.scrollHeight;
  stage.classList.remove("enter");
  void stage.offsetWidth; // restart the entrance
  stage.classList.add("enter");
  input.focus();
}

async function send(text) {
  stopDemo();
  try {
    await call.invoke("send", { text });
  } catch (err) {
    note(open, String(err));
  }
}

// Updates from the conversations

function onUpdate(u) {
  const room = u.chat || open;
  const live = lives[room];
  const here = room === open;
  switch (u.kind) {
    case "team":
      team = { main: u.main, bots: u.bots, groups: u.groups };
      renderRail();
      if (pendingOpen && exists(pendingOpen)) {
        const next = pendingOpen;
        pendingOpen = null;
        openRoom(next);
      }
      break;
    case "you":
      if (demoed.delete(room)) wipe(room); // a real conversation replaces the demo
      if (here) {
        clearHearing();
        setPrint($("you").firstChild, voiceOn ? "listening" : "idle");
      }
      // The files in the tray went with this turn.
      addLine(room, yourLine(u.text, here && !u.demo ? takeTray() : []));
      break;
    case "hearing":
      if (!here) break;
      if (!hearing) {
        hearing = el("div", "line you hearing now");
        hearing.append(whoColumn("You", "").who, el("div", "said"));
        addLine(room, hearing);
      }
      stick(scriptFor(room), () => (hearing.lastChild.textContent = `${u.text}…`));
      setPrint($("you").firstChild, "speaking");
      break;
    case "start":
      lives[room] = botLine(u.bot);
      addLine(room, lives[room].line);
      if (!here) markUnread(room);
      if (here) {
        if (lastSpeaker[room] && !same(lastSpeaker[room], u.bot)) passTurn(lastSpeaker[room], u.bot);
        castState(u.bot, "thinking");
        setPrint($("you").firstChild, voiceOn ? "listening" : "idle");
      }
      lastSpeaker[room] = u.bot;
      break;
    case "text":
      if (!live) break;
      stick(scriptFor(room), () => speak(live.said, u.text));
      if (here && !voiceOn) castState(live.bot, "speaking");
      break;
    case "first_audio":
      if (!live) break;
      live.meta.textContent = `${now()} · ${u.ms} ms`;
      if (here) castState(live.bot, "speaking");
      break;
    case "done":
    case "cut":
      if (live) {
        live.line.classList.remove("live");
        live.line.removeAttribute("aria-busy");
        if (u.kind === "cut") {
          live.line.classList.add("cut");
          if (u.after_ms != null) live.meta.textContent = `stopped in ${u.after_ms} ms`;
        } else if (!live.said.textContent.trim()) {
          live.line.remove(); // nothing was said
        }
        delete lives[room];
      }
      if (here) {
        refreshQuiet();
        castState(null, "idle");
      }
      break;
    case "step":
      live?.steps.append(el("li", "step", u.text));
      if (here) {
        castState(u.bot, "thinking");
        $("status").textContent = u.text;
      }
      break;
    case "sources":
      live?.sources.replaceChildren(...u.sources.map(sourceLink));
      break;
    case "image":
      live?.media.append(picture(u.url, u.prompt));
      break;
    case "task":
      addLine(room, el("p", "notice task", `Reminder for ${u.bot}: ${u.task}`));
      if (!here) markUnread(room);
      break;
    case "problem":
      note(room, u.text);
      break;
  }
}

// Voice

async function setVoice(on) {
  if (on && !voiceReady) {
    openSetup();
    return;
  }
  stopDemo();
  try {
    await call.invoke("voice", { on });
  } catch (err) {
    note(open, String(err));
    return;
  }
  voiceOn = on;
  $("voice").setAttribute("aria-pressed", String(on));
  $("voice").setAttribute("aria-label", on ? "Turn voice off" : "Turn voice on");
  // Spoken lines shouldn't be read out twice by a screen reader.
  Object.values(scripts).forEach((s) => s.setAttribute("aria-live", on ? "off" : "polite"));
  setPrint($("you").firstChild, on ? "listening" : "idle");
  setPrint($("mark").firstChild, on ? "listening" : "idle"); // the logo breathes while Voxen listens
  castState(null, "idle");
}

// A demo: the room talks something over, played on screen only, so you can see Voxen before
// adding keys. Nothing is sent anywhere.

const DEMO_LINES = [
  "Friday: land, drop the bags in Alfama, then sunset at Miradouro de Santa Luzia. Saturday is Belém in the morning and Bairro Alto for dinner. Sunday, Sintra.",
  "Quick reality check: Alfama hotels run about 140 euros a night in spring, so plan on roughly 850 for two, food included. Book Pena Palace early, it sells out.",
  "And if it rains on Saturday, swap Belém for the tile museum. Same side of town, no umbrellas needed.",
];
const DEMO_STEPS = ["Searching the web for “Lisbon long weekend itinerary”", "Searching the web for “Alfama hotel prices in spring”"];
const DEMO_SOURCES = [
  [
    { url: "https://www.visitlisboa.com/en", title: "Visit Lisboa" },
    { url: "https://www.timeout.com/lisbon", title: "Time Out Lisbon" },
  ],
  [
    { url: "https://www.booking.com/city/pt/lisbon.html", title: "Hotels in Lisbon" },
    { url: "https://www.parquesdesintra.pt/en/", title: "Parques de Sintra" },
  ],
];

let demoRoom = ""; // where the demo plays
const demoed = new Set(); // rooms showing a demo, wiped once a real conversation starts there

function stopDemo() {
  if (!demo.length) return;
  demo.forEach(clearTimeout);
  demo = [];
  const live = lives[demoRoom];
  if (live) onUpdate({ chat: demoRoom, kind: "cut", bot: live.bot });
}

function runDemo() {
  stopDemo();
  const room = (demoRoom = open);
  const cast = members(room).length ? members(room) : [team.main];
  let at = 300;
  const later = (ms, fn) => demo.push(setTimeout(fn, (at += ms)));
  later(0, () => {
    onUpdate({ chat: room, kind: "you", text: "Plan a long weekend in Lisbon for two. Mid-range budget.", demo: true });
    demoed.add(room);
  });
  DEMO_LINES.slice(0, Math.max(1, Math.min(3, cast.length))).forEach((text, i) => {
    const who = cast[i % cast.length];
    later(900, () => onUpdate({ chat: room, kind: "start", bot: who }));
    if (DEMO_STEPS[i]) later(400, () => onUpdate({ chat: room, kind: "step", bot: who, text: DEMO_STEPS[i] }));
    later(1100, () => onUpdate({ chat: room, kind: "first_audio", bot: who, ms: 620 + i * 40 }));
    for (const word of text.split(/(?<=\s)/)) later(85, () => onUpdate({ chat: room, kind: "text", bot: who, text: word }));
    if (DEMO_SOURCES[i]) later(200, () => onUpdate({ chat: room, kind: "sources", bot: who, sources: DEMO_SOURCES[i] }));
    later(500, () => onUpdate({ chat: room, kind: "done", bot: who }));
  });
  later(0, () => (demo = []));
}

// Composer

function grow() {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 160)}px`;
  composer.classList.toggle("typed", input.value.trim().length > 0 || tray.length > 0);
}

// Attachments: pictures, PDFs and text files, by button, drop or paste. They go with your
// next turn, typed or spoken.

const PICTURES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const TEXT_FILE = /\.(txt|md|csv|tsv|json|xml|ya?ml|log)$/i;

function readFile(file, asText) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error);
    if (asText) reader.readAsText(file);
    else reader.readAsDataURL(file);
  });
}

async function addFiles(files) {
  for (const file of files) {
    const isPicture = PICTURES.includes(file.type);
    const isPdf = file.type === "application/pdf";
    const isText = !isPicture && !isPdf && (file.type.startsWith("text/") || TEXT_FILE.test(file.name));
    const limit = isPicture ? 5 : 20; // MB that Claude takes
    if (!isPicture && !isPdf && !isText) {
      note(open, `${file.name}: Voxen reads pictures, PDFs and text files.`);
      continue;
    }
    if (file.size > limit * 1024 * 1024) {
      note(open, `${file.name} is over ${limit} MB.`);
      continue;
    }
    const result = await readFile(file, isText).catch(() => null);
    if (result == null) continue;
    tray.push({
      name: file.name,
      media_type: isText ? "text/plain" : file.type,
      data: isText ? result : result.slice(result.indexOf(",") + 1),
      preview: isPicture ? result : "",
    });
  }
  syncTray();
}

// Shows the tray and tells the conversations, so a spoken turn takes the files too.
function syncTray() {
  $("tray").replaceChildren(
    ...tray.map((f, i) => {
      const chip = el("span", "chip-file");
      if (f.preview) chip.append(Object.assign(el("img"), { src: f.preview, alt: "" }));
      chip.append(el("span", "fname", f.name));
      const remove = button("unattach", "×", () => {
        tray.splice(i, 1);
        syncTray();
      });
      remove.setAttribute("aria-label", `Remove ${f.name}`);
      chip.append(remove);
      return chip;
    }),
  );
  grow();
  const files = tray.map(({ name, media_type, data }) => ({ name, media_type, data }));
  call.invoke("attach", { files }).catch(() => {});
}

function takeTray() {
  const sent = tray;
  tray = [];
  syncTray();
  return sent;
}

$("attach").addEventListener("click", () => $("file").click());
$("file").addEventListener("change", (e) => {
  addFiles([...e.target.files]);
  e.target.value = "";
});
input.addEventListener("paste", (e) => {
  const files = [...(e.clipboardData?.files ?? [])];
  if (files.length) {
    e.preventDefault();
    addFiles(files);
  }
});
stage.addEventListener("dragover", (e) => {
  if (![...e.dataTransfer.types].includes("Files")) return;
  e.preventDefault();
  stage.classList.add("dropping");
});
stage.addEventListener("dragleave", (e) => {
  if (!stage.contains(e.relatedTarget)) stage.classList.remove("dropping");
});
stage.addEventListener("drop", (e) => {
  e.preventDefault();
  stage.classList.remove("dropping");
  addFiles([...e.dataTransfer.files]);
});

// Think and Research: how hard the bots work on replies, until you switch back.
function setMode(next) {
  mode = next;
  document.querySelectorAll(".mode").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.mode === mode)));
  call.invoke("mode", { mode }).catch(() => {});
}
document.querySelectorAll(".mode").forEach((b) => b.addEventListener("click", () => setMode(mode === b.dataset.mode ? "quick" : b.dataset.mode)));

// Source links and pictures open in your browser, never inside Voxen.
document.addEventListener("click", (e) => {
  const link = e.target.closest("[data-url]");
  if (!link) return;
  e.preventDefault();
  if (tauri) call.invoke("open_link", { url: link.dataset.url }).catch((err) => note(open, String(err)));
  else window.open(link.dataset.url, "_blank", "noopener");
});

composer.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = input.value.trim() || (tray.length ? "Take a look at this." : "");
  if (!text) return;
  input.value = "";
  grow();
  send(text);
});
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    composer.requestSubmit();
  }
});
input.addEventListener("input", grow);
$("voice").addEventListener("click", () => setVoice(!voiceOn));
$("demo").addEventListener("click", runDemo);
$("clear").addEventListener("click", async () => {
  stopDemo();
  await call.invoke("clear").catch(() => {});
  demoed.delete(open);
  wipe(open);
  input.focus();
});

// Keys: Esc stops whoever is talking, ⌘1–9 open rooms, ⌘D turns voice on or off, ⌘N makes
// a voice and ⇧⌘N a room. Typing anywhere else goes to the composer.
document.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]") || e.isComposing) return;
  if (e.key === "Escape") {
    stopDemo();
    call.invoke("stop").catch(() => {});
    return;
  }
  if (!e.metaKey) {
    if (!e.ctrlKey && !e.altKey && e.key.length === 1 && !e.target.closest("button, input, textarea, select, a")) input.focus();
    return;
  }
  if (e.ctrlKey || e.altKey) return;
  const key = e.key.toLowerCase();
  const room = /^[1-9]$/.test(key) && document.querySelectorAll(".room-btn")[Number(key) - 1];
  if (room) openRoom(room.dataset.room);
  else if (key === "d" && !e.shiftKey) setVoice(!voiceOn);
  else if (key === "n") (e.shiftKey ? openGroupDialog : openBotDialog)();
  else return;
  e.preventDefault();
});

// New voice

let draft = { traits: new Set(), color: HUES[0].hex };
const botField = (name) => $("bot-form").elements.namedItem(name);

function openBotDialog() {
  $("bot-form").reset();
  draft = { traits: new Set(["Warm"]), color: HUES[team.bots.length % HUES.length].hex };
  $("templates").replaceChildren(...TEMPLATES.map((t) => button("toggle", t.label, () => useTemplate(t))));
  renderTraits();
  renderSwatches();
  updatePreview();
  $("speed-out").textContent = "1.0×";
  $("bot-error").textContent = "";
  loadVoices();
  $("bot-dialog").showModal();
  botField("name").focus();
}

async function loadVoices() {
  const fill = (voices) =>
    $("voice-select").replaceChildren(...voices.map((v) => Object.assign(el("option"), { value: v.id, textContent: v.name })));
  fill(FALLBACK_VOICES);
  try {
    const listed = await call.invoke("voices");
    if (listed?.length) fill(listed);
  } catch {
    // no ElevenLabs key yet: the default voices stay
  }
}

function useTemplate(t) {
  if (!taken(t.name)) botField("name").value = t.name;
  botField("role").value = t.role;
  draft.traits = new Set(t.traits);
  draft.color = t.color;
  renderTraits();
  renderSwatches();
  updatePreview();
}

function renderTraits() {
  $("traits").replaceChildren(
    ...TRAITS.map((trait) => {
      const b = button("toggle", trait, () => {
        if (draft.traits.has(trait)) draft.traits.delete(trait);
        else draft.traits.add(trait);
        b.setAttribute("aria-pressed", String(draft.traits.has(trait)));
      });
      b.setAttribute("aria-pressed", String(draft.traits.has(trait)));
      return b;
    }),
  );
}

function renderSwatches() {
  $("swatches").replaceChildren(
    ...HUES.map((h) => {
      const label = el("label", "swatch");
      label.title = h.name;
      const radio = Object.assign(el("input"), { type: "radio", name: "color", value: h.hex, checked: h.hex === draft.color });
      radio.setAttribute("aria-label", h.name);
      radio.addEventListener("change", () => {
        draft.color = h.hex;
        updatePreview();
      });
      const dot = el("span");
      dot.style.setProperty("--c", h.hex);
      label.append(radio, dot);
      return label;
    }),
  );
}

function updatePreview() {
  const name = botField("name").value.trim() || "New voice";
  const print = voiceprint(name, draft.color, 170);
  setPrint(print, "speaking");
  $("preview-print").style.setProperty("--c", draft.color);
  $("preview-print").replaceChildren(print);
  $("preview-name").textContent = name;
  $("preview-role").textContent = botField("role").value.trim() || "What it's for";
}

$("bot-form").addEventListener("input", (e) => {
  $("bot-error").textContent = "";
  if (e.target.name === "speed") $("speed-out").textContent = `${Number(e.target.value).toFixed(2).replace(/0$/, "")}×`;
  if (e.target.name === "name" || e.target.name === "role") updatePreview();
});
$("bot-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const newBot = {
    name: botField("name").value.trim(),
    role: botField("role").value.trim(),
    personality: [...draft.traits].join(", ") || "Friendly and clear",
    voice: botField("voice").value,
    speed: Number(botField("speed").value),
    color: draft.color,
  };
  const error = !newBot.name
    ? "Give your voice a name."
    : same(newBot.name, "you")
      ? "Pick another name: “You” is you."
      : taken(newBot.name)
        ? `${newBot.name} is already taken.`
        : !newBot.role
          ? "Say what it's for."
          : "";
  if (error) {
    $("bot-error").textContent = error;
    return;
  }
  try {
    await call.invoke("add_bot", { bot: newBot });
  } catch (err) {
    $("bot-error").textContent = String(err);
    return;
  }
  pendingOpen = `bot:${newBot.name}`;
  $("bot-dialog").close();
});

// New room

let picked = new Set();

function openGroupDialog() {
  $("group-form").reset();
  $("group-error").textContent = "";
  picked = new Set();
  $("group-bots").replaceChildren(
    ...team.bots.map((b) => {
      const pick = button("pick", "", () => {
        if (picked.has(b.name)) picked.delete(b.name);
        else picked.add(b.name);
        pick.setAttribute("aria-pressed", String(picked.has(b.name)));
        $("group-error").textContent = "";
      });
      pick.setAttribute("aria-pressed", "false");
      const who = el("span", "who-pick");
      who.append(el("b", "", b.name), el("small", "", b.role));
      pick.append(voiceprint(b.name, hue(b.name), 40), who);
      return pick;
    }),
  );
  $("group-dialog").showModal();
}

$("group-form").addEventListener("input", () => ($("group-error").textContent = ""));
$("group-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = $("group-form").elements.namedItem("name").value.trim();
  const bots = team.bots.filter((b) => picked.has(b.name)).map((b) => b.name);
  const error = !name ? "Give the room a name." : taken(name) ? `${name} is already taken.` : bots.length < 2 ? "Pick at least two voices." : "";
  if (error) {
    $("group-error").textContent = error;
    return;
  }
  try {
    await call.invoke("add_group", { group: { name, bots } });
  } catch (err) {
    $("group-error").textContent = String(err);
    return;
  }
  pendingOpen = `group:${name}`;
  $("group-dialog").close();
});

// Setup

function openSetup() {
  $("setup-form").reset();
  $("setup-error").textContent = "";
  $("setup-save").textContent = "Save and restart";
  for (const [id, ok] of [["k-claude", keys.claude], ["k-deepgram", keys.deepgram], ["k-elevenlabs", keys.elevenlabs]]) {
    $(id).textContent = ok ? "Connected" : "Missing";
    $(id).classList.toggle("ok", ok);
  }
  $("k-xai").textContent = keys.xai ? "Connected" : "Optional";
  $("k-xai").classList.toggle("ok", Boolean(keys.xai));
  const print = voiceprint("voxen", ink(), 64, false);
  setPrint(print, "listening");
  $("setup-print").replaceChildren(print);
  $("setup-dialog").showModal();
}

$("setup-form").addEventListener("input", () => ($("setup-error").textContent = ""));
$("setup-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const entered = Object.fromEntries(
    [...new FormData(e.target).entries()].map(([k, v]) => [k, String(v).trim()]).filter(([, v]) => v),
  );
  if (!Object.keys(entered).length) {
    $("setup-error").textContent = "Paste at least one key.";
    return;
  }
  $("setup-save").textContent = "Saving…";
  try {
    await call.invoke("save_keys", { keys: entered }); // Voxen restarts with the new keys
  } catch (err) {
    $("setup-error").textContent = String(err);
    $("setup-save").textContent = "Save and restart";
  }
});

$("setup").addEventListener("click", openSetup);
$("new-bot").addEventListener("click", openBotDialog);
$("new-group").addEventListener("click", openGroupDialog);
document.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => b.closest("dialog").close()));
// A closed dialog's prints stay in the page, so they stop moving.
document.querySelectorAll("dialog").forEach((d) => d.addEventListener("close", () => d.querySelectorAll("canvas").forEach((c) => setPrint(c, "idle"))));

// Start up

function paintMarks() {
  const state = voiceOn ? "listening" : "idle";
  $("mark").replaceChildren(voiceprint("voxen", ink(), 48, false));
  $("you").replaceChildren(voiceprint("you", ink(), 46, false));
  setPrint($("mark").firstChild, state);
  setPrint($("you").firstChild, state);
}

// Ink-coloured prints are drawn once, so they're repainted when the Mac switches light and dark.
matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
  paintMarks();
  renderRail();
});

async function start() {
  let status;
  try {
    status = await call.invoke("status");
  } catch (e) {
    status = { chat: false, voice: false, problem: String(e), keys, main: "", bots: [], groups: [] };
  }
  team = { main: status.main, bots: status.bots, groups: status.groups };
  keys = status.keys;
  voiceReady = status.voice;
  paintMarks();
  renderRail();
  renderSetup();
  call.listen("call", (event) => onUpdate(event.payload));
  await openRoom(team.groups.length && !status.chat ? `group:${team.groups[0].name}` : "main");
  $("note").textContent = status.problem || "";
  if (tauri && !keys.claude) openSetup(); // first run: straight to setup
}

start();
