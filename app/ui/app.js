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
let tray = []; // files waiting to go with your next turn
let providers = []; // every provider a bot can think with, and whether its key is in
let editing = null; // the bot being changed in the voice dialog, or null for a new one
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

// Voiceprints: a ring of bars shaped by a name, so every voice has its own mark. The ones
// on stage are always a little alive: they turn and shimmer at rest, lean in and move with
// your voice (or your typing) while listening, sweep while thinking, and move with the bot's
// voice (or its words arriving) while speaking.

let ticking = false; // whether frames are being drawn
const sound = { you: 0, voice: 0, youAt: 0, voiceAt: 0 }; // the latest loudness, 0 to 1, and when
const heard = { you: 0, voice: 0 }; // the same, smoothed: what the prints show

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

// `alive` prints keep moving at rest; the rest move only while their voice is busy.
function voiceprint(name, color, size, letter = true, alive = false) {
  const rand = seeded(name || "voice");
  const raw = Array.from({ length: 60 }, () => 0.12 + 0.88 * rand() ** 1.5);
  const bars = raw.map((v, i) => (raw[(i + 59) % 60] + v * 2 + raw[(i + 1) % 60]) / 4); // smooth neighbours into one shape
  const canvas = el("canvas", "print");
  const ratio = window.devicePixelRatio || 1;
  canvas.width = canvas.height = Math.round(size * ratio);
  canvas.style.width = canvas.style.height = `${size}px`;
  canvas.setAttribute("aria-hidden", "true");
  const print = { canvas, name, color, size: size * ratio, bars, letter, alive, state: "idle", seed: rand() * 100 };
  canvas.print = print;
  draw(print, performance.now() / 1000);
  if (alive) animate();
  return canvas;
}

// Loudness from the app's meter, a keystroke, or words arriving.
function hear(who, level) {
  sound[who] = level;
  sound[`${who}At`] = performance.now();
  animate();
}

function motion(p, i, n, t) {
  const phase = t + p.seed;
  if (p.state === "speaking") {
    const wobble = Math.abs(Math.sin(phase * 6.5 + i * 0.85) * Math.sin(phase * 2.9 + i * 0.31));
    // With sound to follow, the bars move with it; otherwise they keep a talking rhythm.
    return heard.voice > 0.02 ? 0.4 + heard.voice * (0.3 + 0.6 * wobble) : 0.5 + 0.5 * wobble;
  }
  if (p.state === "thinking") return 0.32 + 0.68 * Math.max(0, Math.cos(phase * 3.2 - (i / n) * Math.PI * 2)) ** 4;
  if (p.state === "listening") {
    const ripple = 0.5 + 0.5 * Math.sin(phase * 7 + i * 1.3) * Math.sin(phase * 3.1 - i * 0.7);
    // Breathes while it waits, and leans in when you talk or type.
    return 0.54 + 0.08 * Math.sin(phase * 1.3 + i * 0.18) + heard.you * 0.55 * ripple;
  }
  // At rest, a slow shimmer, so it never looks switched off.
  return p.alive ? 0.5 + 0.04 * Math.sin(phase * 0.9 + i * 0.45) * Math.sin(phase * 0.37 + i * 0.13) : 0.5;
}

function draw(p, t) {
  const ctx = p.canvas.getContext("2d");
  const s = p.size;
  const c = s / 2;
  const n = p.bars.length;
  const inner = s * (p.letter ? 0.27 : 0.2);
  const reach = s * (p.letter ? 0.25 : 0.3);
  const spin = p.alive && !still.matches ? t * 0.03 : 0; // about one turn every three and a half minutes
  ctx.clearRect(0, 0, s, s);
  ctx.lineCap = "round";
  ctx.lineWidth = Math.max(1, s * (p.letter ? 0.02 : 0.03));
  ctx.strokeStyle = p.color;
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2 - Math.PI / 2 + spin;
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

// Bodies: each bot is an orb of light, a cloud of points shaped by its name. It breathes and
// turns at rest, draws in and shivers with your voice (or your typing) as it listens, swirls
// its bands as it thinks, and sends rings of light outward with its voice as it speaks. Big
// bodies draw on the GPU; small ones, and any Mac without WebGL, draw the same points flat.

const BODY_SCALE = 0.72; // how much of its canvas the orb fills
const BODY_VERT = `
precision highp float;
attribute vec4 aPos;
attribute vec4 aRand;
uniform float uTime, uBreath, uListen, uThink, uSpeak, uLevel, uSize, uDpr, uAlpha, uScale;
uniform vec3 uBase, uAccent;
varying vec3 vColor;
varying float vAlpha;
vec3 rotY(vec3 p, float a) { float c = cos(a), s = sin(a); return vec3(c * p.x + s * p.z, p.y, -s * p.x + c * p.z); }
vec3 rotX(vec3 p, float a) { float c = cos(a), s = sin(a); return vec3(p.x, c * p.y - s * p.z, s * p.y + c * p.z); }
void main() {
  vec3 p = aPos.xyz;
  float loose = step(1.5, aPos.w);
  float bright = step(0.5, aPos.w) * (1.0 - loose);
  p *= 0.84 + 0.05 * uBreath - 0.06 * uListen;
  p = rotY(p, uThink * uTime * (0.5 + (1.0 - abs(p.y)) * 1.8));
  float shiver = sin(dot(aPos.xyz, vec3(9.0, 7.0, 5.0)) + uTime * 7.0 + aRand.x * 6.283);
  p *= 1.0 + uListen * (0.015 + uLevel * 0.15) * shiver;
  p += loose * vec3(sin(uTime * 0.4 + aRand.x * 40.0), cos(uTime * 0.33 + aRand.y * 50.0), sin(uTime * 0.27 + aRand.z * 60.0)) * 0.12;
  p = rotY(p, uTime * (0.18 + uSpeak * 0.14));
  p = rotX(p, 0.32);
  float ring = 0.5 + 0.5 * sin(length(p.xy) * 11.0 - uTime * 11.0);
  p.xy *= 1.0 + uSpeak * (0.03 + uLevel * 0.24) * ring;
  float persp = 2.6 / (2.6 - p.z);
  gl_Position = vec4(p.xy * persp * uScale, 0.0, 1.0);
  gl_PointSize = uSize * uDpr * persp * (0.6 + aRand.z * 0.8) * (1.0 + bright * 0.6);
  vColor = mix(uBase, uAccent, bright);
  float front = 0.45 + 0.55 * smoothstep(-0.9, 0.9, p.z);
  float twinkle = 0.7 + 0.3 * sin(uTime * (1.0 + aRand.w * 2.0) + aRand.w * 30.0);
  float lift = 1.0 + uListen * (0.25 + uLevel * 0.4) + uSpeak * (0.2 + uLevel * 0.6) + uThink * 0.15;
  vAlpha = uAlpha * front * twinkle * lift * (0.75 + 0.25 * aRand.y) * (1.0 - loose * 0.5);
}`;
// Solid round dots in premultiplied colour, drawn over each other rather than added up, so
// every bot keeps its own deep colour instead of washing out to white.
const BODY_FRAG = `
precision mediump float;
varying vec3 vColor;
varying float vAlpha;
void main() {
  vec2 c = gl_PointCoord - 0.5;
  float a = (1.0 - smoothstep(0.45, 1.0, dot(c, c) * 4.0)) * min(vAlpha, 1.0);
  if (a < 0.003) discard;
  gl_FragColor = vec4(vColor * a, a);
}`;

// The points of a body: a sphere whose bumps and ridges come from the name, a few bright
// points, and a few loose ones drifting round it.
function bodyPoints(name, count) {
  const rand = seeded(`${name || "voice"} body`);
  const golden = Math.PI * (3 - Math.sqrt(5));
  const lobes = 2 + Math.floor(rand() * 4), ridges = 2 + rand() * 4, depth = 0.04 + rand() * 0.07, twist = rand() * 6.283;
  const pos = new Float32Array(count * 4), r4 = new Float32Array(count * 4);
  for (let i = 0; i < count; i++) {
    const y = 1 - (i / (count - 1)) * 2, ring = Math.sqrt(1 - y * y), th = golden * i;
    const loose = rand() < 0.05;
    const rr = (0.86 + depth * Math.sin(th * lobes + twist) * Math.cos(y * ridges) + (rand() - 0.5) * 0.03) * (loose ? 1.2 + rand() * 0.45 : 1);
    const o = i * 4;
    pos[o] = Math.cos(th) * ring * rr;
    pos[o + 1] = y * rr;
    pos[o + 2] = Math.sin(th) * ring * rr;
    pos[o + 3] = loose ? 2 : rand() < 0.16 ? 1 : 0;
    for (let k = 0; k < 4; k++) r4[o + k] = rand();
  }
  return { pos, rand: r4, count, seed: rand() * 100 };
}

function glPainter(canvas) {
  const gl = canvas.getContext("webgl", { alpha: true, premultipliedAlpha: true, antialias: false });
  if (!gl) return null;
  const shader = (type, source) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, source);
    gl.compileShader(s);
    return gl.getShaderParameter(s, gl.COMPILE_STATUS) ? s : null;
  };
  const vs = shader(gl.VERTEX_SHADER, BODY_VERT), fs = shader(gl.FRAGMENT_SHADER, BODY_FRAG);
  if (!vs || !fs) return null;
  const program = gl.createProgram();
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
  gl.useProgram(program);
  const attribute = (name) => {
    const buffer = gl.createBuffer();
    const at = gl.getAttribLocation(program, name);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.enableVertexAttribArray(at);
    gl.vertexAttribPointer(at, 4, gl.FLOAT, false, 0, 0);
    return buffer;
  };
  const buffers = { pos: attribute("aPos"), rand: attribute("aRand") };
  const names = ["uTime", "uBreath", "uListen", "uThink", "uSpeak", "uLevel", "uSize", "uDpr", "uAlpha", "uScale", "uBase", "uAccent"];
  const u = Object.fromEntries(names.map((n) => [n, gl.getUniformLocation(program, n)]));
  let count = 0;
  return {
    load(data) {
      gl.bindBuffer(gl.ARRAY_BUFFER, buffers.pos);
      gl.bufferData(gl.ARRAY_BUFFER, data.pos, gl.STATIC_DRAW);
      gl.bindBuffer(gl.ARRAY_BUFFER, buffers.rand);
      gl.bufferData(gl.ARRAY_BUFFER, data.rand, gl.STATIC_DRAW);
      count = data.count;
    },
    draw(f) {
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      const one = { uTime: f.t, uBreath: f.breath, uListen: f.listen, uThink: f.think, uSpeak: f.speak, uLevel: f.level, uSize: f.size, uDpr: f.dpr, uAlpha: f.alpha, uScale: BODY_SCALE };
      for (const [key, value] of Object.entries(one)) gl.uniform1f(u[key], value);
      gl.uniform3fv(u.uBase, f.base);
      gl.uniform3fv(u.uAccent, f.accent);
      gl.drawArrays(gl.POINTS, 0, count);
    },
    dispose() {
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    },
  };
}

// The same body drawn with the 2D canvas: for small icons, where a GPU context per icon would
// cost more than it shows, and for any Mac without WebGL.
function flatPainter(canvas) {
  const ctx = canvas.getContext("2d");
  let data = null;
  return {
    load(d) {
      data = d;
    },
    draw(f) {
      const s = canvas.width, half = s / 2, scale = half * BODY_SCALE * (f.zoom ?? 1), dot = Math.max(1.5, s / 80);
      const ox = half + (f.at?.[0] ?? 0) * half, oy = half + (f.at?.[1] ?? 0) * half;
      if (!f.keep) ctx.clearRect(0, 0, s, s);
      const yaw = f.t * (0.18 + f.speak * 0.14 + f.think * 0.8), cy = Math.cos(yaw), sy = Math.sin(yaw), cx = Math.cos(0.32), sx = Math.sin(0.32);
      const grow = 0.84 + 0.05 * f.breath - 0.06 * f.listen;
      for (const [bright, colour] of [[false, f.base], [true, f.accent]]) {
        ctx.fillStyle = `rgb(${colour.map((v) => Math.round(v * 255)).join(",")})`;
        for (let i = 0; i < data.count; i++) {
          const o = i * 4, w = data.pos[o + 3];
          if ((w > 0.5 && w < 1.5) !== bright) continue;
          let k = grow;
          if (f.listen > 0.01) k *= 1 + f.listen * (0.015 + f.level * 0.15) * Math.sin(data.pos[o] * 9 + data.pos[o + 1] * 7 + data.pos[o + 2] * 5 + f.t * 7 + data.rand[o] * 6.283);
          const x = data.pos[o] * k, y = data.pos[o + 1] * k, z = data.pos[o + 2] * k;
          const X = cy * x + sy * z, Z0 = -sy * x + cy * z, Y = cx * y - sx * Z0, Z = sx * y + cx * Z0;
          const ripple = f.speak > 0.01 ? 1 + f.speak * (0.03 + f.level * 0.24) * (0.5 + 0.5 * Math.sin(Math.hypot(X, Y) * 11 - f.t * 11)) : 1;
          const persp = 2.6 / (2.6 - Z);
          ctx.globalAlpha = Math.min(1, f.alpha * (0.45 + (0.55 * (Z + 0.9)) / 1.8) * (0.75 + 0.25 * data.rand[o + 1]) * (w > 1.5 ? 0.5 : 1));
          ctx.fillRect(ox + X * ripple * persp * scale, oy - Y * ripple * persp * scale, dot, dot);
        }
      }
      ctx.globalAlpha = 1;
    },
    dispose() {},
  };
}

const lightScheme = matchMedia("(prefers-color-scheme: light)");
const rgbOf = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255 || 0);

// `alive` bodies keep moving at rest; `gpu` false keeps a big body on the flat painter.
function body(name, color, size, alive = false, gpu = true) {
  const canvas = el("canvas", "print body");
  const ratio = Math.min(window.devicePixelRatio || 1, 1.75);
  canvas.width = canvas.height = Math.round(size * ratio);
  canvas.style.width = canvas.style.height = `${size}px`;
  canvas.setAttribute("aria-hidden", "true");
  const big = size >= 90;
  const count = Math.round(size * (big ? 13 : 7));
  const p = { canvas, name, color, size, alive, body: true, state: "idle", ratio, seed: 0, mix: { listen: 0, think: 0, speak: 0 } };
  p.painter = (big && gpu && glPainter(canvas)) || flatPainter(canvas);
  // A new name or colour reshapes the same body, so the New voice preview keeps one canvas.
  p.retarget = (nextName, nextColor) => {
    const data = bodyPoints(nextName, count);
    Object.assign(p, { name: nextName, color: nextColor, seed: data.seed });
    p.painter.load(data);
  };
  p.retarget(name, color);
  canvas.print = p;
  paint(p, performance.now() / 1000);
  if (alive) animate();
  return canvas;
}

// A group's icon: up to three of its voices' orbs overlapping, each in its own colour, so a
// group reads as a group at a glance. Where they overlap, their dots mix.
function groupBody(group, size) {
  const canvas = el("canvas", "print body");
  const ratio = Math.min(window.devicePixelRatio || 1, 1.75);
  canvas.width = canvas.height = Math.round(size * ratio);
  canvas.style.width = canvas.style.height = `${size}px`;
  canvas.setAttribute("aria-hidden", "true");
  const voices = group.bots.slice(0, 3);
  const spots = voices.length < 3 ? [[-0.2, 0], [0.2, 0]] : [[0, -0.19], [-0.2, 0.15], [0.2, 0.15]];
  const parts = voices.map((name, i) => {
    const painter = flatPainter(canvas);
    painter.load(bodyPoints(name, Math.round(size * 5)));
    return { painter, color: hue(name), at: spots[i], zoom: 0.6 };
  });
  const p = { canvas, name: group.name, size, alive: false, body: true, state: "idle", ratio, seed: 0, mix: { listen: 0, think: 0, speak: 0 }, parts };
  canvas.print = p;
  paint(p, performance.now() / 1000);
  return canvas;
}

// Frees the GPU contexts of the bodies inside `container` before they're replaced.
function retire(container) {
  container.querySelectorAll("canvas.body").forEach((c) => c.print.painter.dispose());
}

function paintBody(p, t) {
  for (const [key, state] of [["listen", "listening"], ["think", "thinking"], ["speak", "speaking"]]) {
    p.mix[key] += ((p.state === state ? 1 : 0) - p.mix[key]) * 0.1;
  }
  if (!p.alive && p.state === "idle") p.mix = { listen: 0, think: 0, speak: 0 }; // a still icon rests fully
  const talking = heard.voice > 0.02 ? heard.voice : 0.3 + 0.35 * Math.abs(Math.sin(t * 8.3) * Math.sin(t * 3.1));
  const level = p.state === "listening" ? heard.you : p.state === "speaking" ? talking : 0;
  const light = lightScheme.matches;
  // One orb, or a group's several drawn over each other on the same canvas.
  const parts = p.parts ?? [{ painter: p.painter, color: p.color }];
  parts.forEach((part, i) => {
    const rgb = rgbOf(part.color);
    part.painter.draw({
      t,
      breath: 0.5 + 0.5 * Math.sin(t * 1.2 + p.seed + i * 2.1),
      level,
      ...p.mix,
      base: rgb.map((v) => v * (light ? 0.52 : 0.85)),
      accent: light ? rgb.map((v) => v * 0.32) : rgb.map((v) => v + (1 - v) * 0.25),
      alpha: 1,
      size: Math.max(1.3, p.size / 85),
      dpr: p.ratio,
      at: part.at,
      zoom: part.zoom,
      keep: i > 0,
    });
  });
}

function paint(p, t) {
  if (p.body) paintBody(p, t);
  else draw(p, t);
}

function setPrint(canvas, state) {
  const p = canvas?.print;
  if (!p || p.state === state) return;
  p.state = state;
  paint(p, performance.now() / 1000);
  if (state !== "idle") animate();
}

function animate() {
  if (ticking || still.matches) return;
  ticking = true;
  requestAnimationFrame(frame);
}

let glow = { you: -1, talk: -1 }; // what the cast's glow was last set to
function frame(ms) {
  // Loudness that stopped coming fades out; new loudness comes in fast.
  for (const who of ["you", "voice"]) {
    const target = ms - sound[`${who}At`] < 250 ? sound[who] : 0;
    heard[who] += (target - heard[who]) * (target > heard[who] ? 0.5 : 0.08);
  }
  // The glow behind a voice follows the same loudness; a talking rhythm stands in when silent.
  const next = { you: heard.you, talk: heard.voice > 0.02 ? heard.voice : 0.3 + 0.2 * Math.sin(ms / 310) };
  for (const [key, value] of Object.entries(next)) {
    if (Math.abs(value - glow[key]) > 0.01) {
      glow[key] = value;
      $("cast").style.setProperty(`--${key}`, value.toFixed(3));
    }
  }
  let moving = false;
  for (const canvas of document.querySelectorAll("canvas.print")) {
    const p = canvas.print;
    if (still.matches || (p.state === "idle" && !p.alive)) continue;
    moving = true;
    if (p.state === "idle" && ms - (p.drawn ?? 0) < 33) continue; // a calm 30 frames a second at rest
    p.drawn = ms;
    paint(p, ms / 1000);
  }
  ticking = moving || heard.you > 0.01 || heard.voice > 0.01;
  if (ticking) requestAnimationFrame(frame);
}

// While you type, the room's voices listen in, and their bars move with your keys.
let typingTimer = 0;
function attend(state) {
  document.querySelectorAll("#cast .member").forEach((card) => {
    if (card.dataset.state === "thinking" || card.dataset.state === "speaking") return;
    card.dataset.state = state;
    setPrint(card.querySelector("canvas"), state);
  });
  setPrint($("you").firstChild, state);
}
function typing() {
  hear("you", 0.35 + Math.random() * 0.35);
  if (voiceOn) return;
  attend("listening");
  clearTimeout(typingTimer);
  typingTimer = setTimeout(() => !voiceOn && attend("idle"), 1600);
}

// A room's icon in the sidebar moves while a voice works there.
function railState(room, state) {
  setPrint(document.querySelector(`.room-btn[data-room="${CSS.escape(room)}"] canvas`), state);
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
  const b = button("room-btn", "", () => goRoom(room));
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
  if (mainBot) items.push(roomButton("main", body(mainBot.name, hue(mainBot.name), 42), mainBot.name, "Your main voice"));
  for (const b of team.bots.filter((x) => !same(x.name, team.main))) {
    items.push(roomButton(`bot:${b.name}`, body(b.name, hue(b.name), 42), b.name, b.role));
  }
  for (const g of team.groups) {
    items.push(roomButton(`group:${g.name}`, groupBody(g, 42), g.name, and(g.bots), g.bots.length));
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

// Who thinks for a bot: its provider, and the model shown for it.
const brainOf = (b) => providers.find((p) => p.id === (b?.provider || "anthropic"));
const brainLabel = (b) => {
  const p = brainOf(b);
  return p ? `${p.name}, ${b.model || p.model || "no model chosen"}` : "";
};

function renderSetup() {
  const chat = Boolean(brainOf(bot(team.main))?.ready);
  const missing = [chat, keys.DEEPGRAM_API_KEY, keys.ELEVENLABS_API_KEY].filter((ok) => !ok).length;
  $("settings-btn").classList.toggle("ok", missing === 0);
  $("settings-sub").textContent = missing === 0 ? "Keys, bots and the look" : chat ? `Add ${missing} voice key${missing > 1 ? "s" : ""}` : "Add your keys";
}

// The cast

function renderCast() {
  const names = members(open);
  const size = names.length === 1 ? 200 : names.length === 2 ? 160 : 128;
  retire($("cast"));
  $("cast").replaceChildren(
    ...names.map((name, i) => {
      const card = el("div", "member");
      card.dataset.name = name;
      card.style.setProperty("--c", hue(name));
      const ring = el("span", "glow");
      ring.append(body(name, hue(name), size, true, i < 4)); // a few GPU bodies at most
      card.append(ring, el("span", "n", name), el("span", "s"));
      card.title = `Change ${name}: brain, voice and more`;
      card.addEventListener("click", () => bot(name) && openBotDialog(bot(name)));
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

// The room's name, who's in it, and for one bot, what it thinks with.
function renderHeader() {
  const names = members(open);
  const k = kind(open);
  const one = k === "group" ? null : bot(names[0]);
  $("room-title").textContent = k === "group" ? open.slice(6) : names[0] || "";
  $("room-sub").textContent =
    k === "group" ? `${and(names)} and you` : [k === "main" ? "Main voice" : "", one?.role, brainLabel(one)].filter(Boolean).join(" · ");
  $("edit").hidden = !one;
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
  renderHeader();
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
      renderHeader();
      renderSetup();
      refreshSettings();
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
      setPrint($("you").firstChild, "listening");
      break;
    case "start":
      lives[room] = botLine(u.bot);
      addLine(room, lives[room].line);
      railState(room, "thinking");
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
      railState(room, "speaking");
      if (here && !voiceOn) {
        castState(live.bot, "speaking");
        hear("voice", 0.4 + Math.random() * 0.35); // no audio to follow, so the words set the pace
      }
      break;
    case "first_audio":
      if (!live) break;
      live.meta.textContent = `${now()} · ${u.ms} ms`;
      if (here) castState(live.bot, "speaking");
      break;
    case "done":
    case "cut":
      railState(room, "idle");
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
    case "levels":
      hear("you", u.you);
      hear("voice", u.voice);
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
      addLine(room, el("p", "notice task", `${u.bot}'s task: ${u.task}`));
      if (!here) markUnread(room);
      break;
    case "problem":
      note(room, u.text);
      if (inSettings()) $("settings-note").textContent = u.text;
      break;
  }
}

// Voice

async function setVoice(on) {
  if (on && !voiceReady) {
    openSettings("keys");
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
  // You ask out loud first: the voices listen, and your words appear as you say them.
  const ask = "Plan a long weekend in Lisbon for two. Mid-range budget.";
  const words = ask.split(" ");
  later(0, () => attend("listening"));
  words.forEach((_, k) =>
    later(140, () => {
      onUpdate({ chat: room, kind: "hearing", text: words.slice(0, k + 1).join(" ") });
      hear("you", 0.35 + Math.random() * 0.45);
    }),
  );
  later(400, () => {
    onUpdate({ chat: room, kind: "you", text: ask, demo: true });
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
input.addEventListener("input", typing);
$("voice").addEventListener("click", () => setVoice(!voiceOn));
$("demo").addEventListener("click", runDemo);
$("clear").addEventListener("click", async () => {
  stopDemo();
  await call.invoke("clear").catch(() => {});
  demoed.delete(open);
  wipe(open);
  input.focus();
});

// Keys: Esc stops whoever is talking (and leaves Settings), ⌘1–9 open rooms, ⌘D turns voice
// on or off, ⌘N makes a voice, ⇧⌘N a room and ⌘, opens Settings. Typing anywhere else in a
// room goes to the composer.
document.addEventListener("keydown", (e) => {
  if (document.querySelector("dialog[open]") || e.isComposing) return;
  if (e.key === "Escape") {
    stopDemo();
    call.invoke("stop").catch(() => {});
    closeSettings();
    return;
  }
  if (!e.metaKey) {
    if (!inSettings() && !e.ctrlKey && !e.altKey && e.key.length === 1 && !e.target.closest("button, input, textarea, select, a")) input.focus();
    return;
  }
  if (e.ctrlKey || e.altKey) return;
  const key = e.key.toLowerCase();
  const room = /^[1-9]$/.test(key) && document.querySelectorAll(".room-btn")[Number(key) - 1];
  if (room) goRoom(room.dataset.room);
  else if (key === ",") openSettings();
  else if (key === "d" && !e.shiftKey) setVoice(!voiceOn);
  else if (key === "n") (e.shiftKey ? openGroupDialog : openBotDialog)();
  else return;
  e.preventDefault();
});

// New voice

let draft = { traits: new Set(), color: HUES[0].hex };
const botField = (name) => $("bot-form").elements.namedItem(name);

// Opens the voice dialog for a new bot, or with `existing` to change one (the name stays).
function openBotDialog(existing) {
  editing = existing?.name ? existing : null;
  $("bot-form").reset();
  const words = editing ? editing.personality.split(/,\s*/).filter(Boolean) : ["Warm"];
  draft = {
    traits: new Set(words.filter((w) => TRAITS.includes(w))),
    extra: words.filter((w) => !TRAITS.includes(w)), // personality words that aren't chips are kept
    color: editing?.color ?? HUES[team.bots.length % HUES.length].hex,
  };
  $("templates").replaceChildren(...TEMPLATES.map((t) => button("toggle", t.label, () => useTemplate(t))));
  $("templates").parentElement.hidden = Boolean(editing);
  $("bot-title").textContent = editing ? `Change ${editing.name}` : "New voice";
  $("bot-save").textContent = editing ? "Save changes" : "Create voice";
  botField("name").readOnly = Boolean(editing);
  if (editing) {
    botField("name").value = editing.name;
    botField("role").value = editing.role;
    botField("speed").value = editing.speed;
  }
  renderTraits();
  renderSwatches();
  renderBrains(editing?.provider || bot(team.main)?.provider || "anthropic", editing?.model ?? "");
  updatePreview();
  $("speed-out").textContent = `${Number(botField("speed").value).toFixed(2).replace(/0$/, "")}×`;
  $("bot-error").textContent = "";
  loadVoices(editing?.voice);
  $("bot-dialog").showModal();
  (editing ? botField("role") : botField("name")).focus();
}

// Thinks with: the provider and model for this bot. Models load live from the provider.
function renderBrains(chosen, model) {
  $("provider-select").replaceChildren(
    ...providers.map((p) => Object.assign(el("option"), { value: p.id, textContent: p.ready ? p.name : `${p.name} (add its key)` })),
  );
  $("provider-select").value = chosen;
  loadModels(model);
}

let modelsAsked = 0; // the latest list asked for wins
async function loadModels(model) {
  const p = brainOf({ provider: $("provider-select").value });
  const field = $("model-input");
  field.value = model || p?.model || "";
  field.placeholder = p?.model || "Type the model's name";
  const fill = (names) => $("model-options").replaceChildren(...names.map((n) => Object.assign(el("option"), { value: n })));
  fill(p?.model ? [p.model] : []);
  const can = p ? [p.web ? "Searches the web" : "No web search", p.tools ? "uses memory and reminders" : "no memory or reminders"].join(", ") : "";
  $("brain-note").textContent = !p ? "" : p.ready ? `${can}.` : `Add your ${p.name} key in Settings to use it. ${can}.`;
  if (!p?.ready) return;
  const asked = ++modelsAsked;
  try {
    const names = await call.invoke("models", { provider: p.id });
    if (asked !== modelsAsked || !names?.length) return;
    fill(names);
    if (!field.value) field.value = names[0];
  } catch {
    // the list couldn't load; typing a model's name still works
  }
}
$("provider-select").addEventListener("change", () => loadModels(""));

async function loadVoices(selected) {
  const fill = (voices) => {
    const list = selected && !voices.some((v) => v.id === selected) ? [...voices, { id: selected, name: "Its current voice" }] : voices;
    $("voice-select").replaceChildren(...list.map((v) => Object.assign(el("option"), { value: v.id, textContent: v.name })));
    if (selected) $("voice-select").value = selected;
  };
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
  const holder = $("preview-print");
  let print = holder.firstChild?.print?.body ? holder.firstChild : null;
  if (print) print.print.retarget(name, draft.color);
  else holder.replaceChildren((print = body(name, draft.color, 160, true)));
  setPrint(print, "speaking");
  holder.style.setProperty("--c", draft.color);
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
    name: editing ? editing.name : botField("name").value.trim(),
    role: botField("role").value.trim(),
    personality: [...draft.traits, ...draft.extra].join(", ") || "Friendly and clear",
    voice: botField("voice").value,
    speed: Number(botField("speed").value),
    color: draft.color,
    provider: $("provider-select").value || "anthropic",
    model: $("model-input").value.trim(),
  };
  const error = editing
    ? !newBot.role
      ? "Say what it's for."
      : ""
    : !newBot.name
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
    await call.invoke(editing ? "update_bot" : "add_bot", { bot: newBot });
  } catch (err) {
    $("bot-error").textContent = String(err);
    return;
  }
  if (!editing) pendingOpen = `bot:${newBot.name}`;
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
      pick.append(body(b.name, hue(b.name), 32), who);
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

// Settings: a page of its own beside the rail. Each section draws itself from the latest team,
// keys and memory when it's shown, and again when they change.

const settings = $("settings");
const panes = { keys: renderKeys };
let pane = "keys";
const inSettings = () => !settings.hidden;

function openSettings(to = pane) {
  pane = panes[to] ? to : "keys";
  stage.hidden = true;
  settings.hidden = false;
  $("settings-btn").setAttribute("aria-current", "page");
  document.querySelectorAll(".room-btn").forEach((r) => r.removeAttribute("aria-current"));
  document.querySelectorAll("#set-nav button").forEach((b) => {
    if (b.dataset.pane === pane) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  });
  document.querySelectorAll(".pane").forEach((p) => (p.hidden = p.dataset.pane !== pane));
  $("settings-note").textContent = "";
  panes[pane]();
}

function closeSettings() {
  if (!inSettings()) return;
  settings.hidden = true;
  stage.hidden = false;
  $("settings-btn").removeAttribute("aria-current");
  markOpen();
}

function refreshSettings() {
  if (inSettings()) panes[pane]();
}

// Leaving Settings for a room, from the rail or ⌘1–9.
function goRoom(room) {
  closeSettings();
  return openRoom(room);
}

$("set-nav").addEventListener("click", (e) => {
  const to = e.target.closest("button")?.dataset.pane;
  if (to) openSettings(to);
});
$("settings-done").addEventListener("click", closeSettings);
$("setup-cta").addEventListener("click", () => openSettings("keys"));

// Keys

const VOICE_KEYS = [
  { key: "DEEPGRAM_API_KEY", name: "Deepgram", help: "Listens when you talk. Needed for voice. Get a key at console.deepgram.com." },
  { key: "ELEVENLABS_API_KEY", name: "ElevenLabs", help: "Gives every voice its sound. Needed for voice. Get a key at elevenlabs.io under Developers." },
];

// One service in Keys: its name, whether it's connected, what it's for, and where its key goes.
function keyRow(name, state, help, fields) {
  const row = el("div", "key");
  const head = el("div", "key-head");
  const pill = { connected: ["pill ok", "Connected"], missing: ["pill", "Missing"], optional: ["pill optional", "Optional"], local: ["pill optional", "No key needed"] }[state];
  head.append(el("span", "key-name", name), el("span", pill[0], pill[1]));
  row.append(head, el("p", "key-help", help));
  for (const [field, placeholder, type = "password"] of fields) {
    row.append(Object.assign(el("input"), { name: field, type, placeholder, autocomplete: "off", spellcheck: false }));
    row.lastChild.setAttribute("aria-label", `${name}: ${placeholder}`);
  }
  return row;
}

function renderKeys() {
  // Keys you've pasted but not saved stay put while the rows' states redraw around them.
  if ([...new FormData($("keys-form")).values()].some((v) => String(v).trim())) return;
  $("keys-error").textContent = "";
  $("keys-save").textContent = "Save and restart";
  $("voice-keys").replaceChildren(
    ...VOICE_KEYS.map((k) => keyRow(k.name, keys[k.key] ? "connected" : "missing", k.help, [[k.key, `Paste your ${k.name} key`]])),
  );
  const thinker = brainOf(bot(team.main));
  $("brain-keys").replaceChildren(
    ...providers.map((p) => {
      const does = [p.web && "Searches the web.", p.id === "xai" && "Also gives every bot X search and Grok Imagine pictures."].filter(Boolean).join(" ");
      if (!p.key) return keyRow(p.name, "local", `Runs models on this Mac. Install it from ${p.site}.`, []);
      const state = p.ready ? "connected" : p === thinker ? "missing" : "optional";
      const help = `${does} Get a key at ${p.site}.`.trim();
      if (p.id === "custom") {
        return keyRow(p.name, state, "Any server that speaks the OpenAI API: its address, and a key if it needs one.", [
          ["CUSTOM_BASE_URL", "https://your-server/v1", "url"],
          [p.key, "Its key (if it needs one)"],
        ]);
      }
      return keyRow(p.name, state, help, [[p.key, `Paste your ${p.name} key`]]);
    }),
  );
}

$("keys-form").addEventListener("input", () => ($("keys-error").textContent = ""));
$("keys-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const entered = Object.fromEntries(
    [...new FormData(e.target).entries()].map(([k, v]) => [k, String(v).trim()]).filter(([, v]) => v),
  );
  if (!Object.keys(entered).length) {
    $("keys-error").textContent = "Paste at least one key.";
    return;
  }
  $("keys-save").textContent = "Saving…";
  try {
    await call.invoke("save_keys", { keys: entered }); // Voxen restarts with the new keys
  } catch (err) {
    $("keys-error").textContent = String(err);
    $("keys-save").textContent = "Save and restart";
  }
});

$("settings-btn").addEventListener("click", () => openSettings());
$("new-bot").addEventListener("click", () => openBotDialog());
$("edit").addEventListener("click", () => openBotDialog(bot(members(open)[0])));
$("new-group").addEventListener("click", openGroupDialog);
document.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => b.closest("dialog").close()));
// A closed dialog's prints stay in the page, so they stop moving.
document.querySelectorAll("dialog").forEach((d) => d.addEventListener("close", () => d.querySelectorAll("canvas").forEach((c) => setPrint(c, "idle"))));

// Start up

function paintMarks() {
  const state = voiceOn ? "listening" : "idle";
  $("mark").replaceChildren(voiceprint("voxen", ink(), 48, false, true));
  $("you").replaceChildren(voiceprint("you", ink(), 46, false, true));
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
    status = { chat: false, voice: false, problem: String(e), keys: {}, providers: [], main: "", bots: [], groups: [] };
  }
  team = { main: status.main, bots: status.bots, groups: status.groups };
  keys = status.keys;
  providers = status.providers;
  voiceReady = status.voice;
  paintMarks();
  renderRail();
  renderSetup();
  call.listen("call", (event) => onUpdate(event.payload));
  await openRoom(team.groups.length && !status.chat ? `group:${team.groups[0].name}` : "main");
  $("note").textContent = status.problem || "";
  // No pop-up on first run: the empty room says what's missing and offers the way to add it.
  $("setup-cta").hidden = !(tauri && !status.chat);
}

start();
