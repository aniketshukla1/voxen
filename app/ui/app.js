// The window: chats with your main bot, your own bots and group chats, shown like a chat app,
// with voice one click away.

const tauri = window.__TAURI__;
const call = tauri
  ? { invoke: tauri.core.invoke, listen: tauri.event.listen }
  : { invoke: () => Promise.reject("Open this in the Voice Team app to chat."), listen: () => {} };

const COLORS = ["#9d95ff", "#4fd1b5", "#ff9b73", "#f2b84b", "#7cc4ff", "#f48fb1"];
// Offered when your voice list can't be fetched (no ElevenLabs key yet).
const FALLBACK_VOICES = [
  { id: "EXAVITQu4vr4xnSDxMaL", name: "Sarah" },
  { id: "JBFqnCBsd6RMkjVDRZzb", name: "George" },
];
const $ = (id) => document.getElementById(id);
const main = $("main");
const input = $("input");
const voicebar = $("voicebar");

let team = { main: "", bots: [], groups: [] };
let open = "main";
let voiceOn = false;
let pendingOpen = null; // a chat to open once its new bot or group shows up
let hearing = null; // what you're saying, before your turn ends
const threads = {}; // chat id → its thread
const lives = {}; // chat id → the bot reply being written there

const same = (a, b) => a.trim().toLowerCase() === b.trim().toLowerCase();
const bot = (name) => team.bots.find((b) => same(b.name, name));
const group = (name) => team.groups.find((g) => same(g.name, name));
const taken = (name) => Boolean(bot(name) || group(name));
const color = (name) => COLORS[Math.max(0, team.bots.findIndex((b) => same(b.name, name))) % COLORS.length];
const and = (names) => (names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names.at(-1)}` : names[0] || "");

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function avatar(name) {
  const a = el("span", "avatar", (name[0] || "?").toUpperCase());
  a.style.background = color(name);
  a.setAttribute("aria-hidden", "true");
  return a;
}

function members(chat) {
  if (chat.startsWith("bot:")) return [chat.slice(4)];
  if (chat.startsWith("group:")) return group(chat.slice(6))?.bots ?? [];
  return [team.main];
}

function exists(chat) {
  if (chat.startsWith("bot:")) return Boolean(bot(chat.slice(4)));
  if (chat.startsWith("group:")) return Boolean(group(chat.slice(6)));
  return true;
}

function describe(chat) {
  const names = members(chat);
  if (chat.startsWith("group:")) {
    return { title: chat.slice(6), sub: `${names.join(", ")} and you`, hero: `Ask ${and(names)} together`, placeholder: "Message the group" };
  }
  const b = bot(names[0]) ?? { name: names[0] ?? "", role: "" };
  const sub = chat === "main" ? `Main bot · ${b.role}` : b.role;
  const hero = chat === "main" ? `What can ${b.name} do for you?` : `Chat with ${b.name}`;
  return { title: b.name, sub, hero, placeholder: `Message ${b.name}` };
}

// Sidebar

function row(chat, faces, name, sub) {
  const button = el("button", "row");
  button.dataset.chat = chat;
  const stack = el("span", "stack");
  faces.forEach((face) => stack.append(avatar(face)));
  const who = el("span", "who");
  who.append(el("span", "", name), el("span", "role", sub));
  button.append(stack, who);
  button.addEventListener("click", () => openChat(chat));
  const li = el("li");
  li.append(button);
  return li;
}

function renderSidebar() {
  const mainBot = bot(team.main);
  $("main-list").replaceChildren(...(mainBot ? [row("main", [mainBot.name], mainBot.name, "Main bot")] : []));
  const others = team.bots.filter((b) => !same(b.name, team.main));
  $("bot-list").replaceChildren(...others.map((b) => row(`bot:${b.name}`, [b.name], b.name, b.role)));
  $("group-list").replaceChildren(...team.groups.map((g) => row(`group:${g.name}`, g.bots.slice(0, 3), g.name, g.bots.join(", "))));
  markOpen();
}

function markOpen() {
  document.querySelectorAll(".row").forEach((r) => {
    if (r.dataset.chat === open) r.setAttribute("aria-current", "page");
    else r.removeAttribute("aria-current");
  });
}

// Threads

function threadFor(chat) {
  if (!threads[chat]) {
    const thread = el("div", "thread");
    thread.setAttribute("role", "log");
    thread.setAttribute("aria-live", voiceOn ? "off" : "polite");
    thread.setAttribute("aria-relevant", "additions");
    thread.hidden = chat !== open;
    $("threads").append(thread);
    threads[chat] = thread;
  }
  return threads[chat];
}

function refreshEmpty() {
  main.classList.toggle("empty", !threadFor(open).childElementCount);
}

// Runs a change to a thread, keeping it scrolled to the end if it already was.
function stickToBottom(thread, change) {
  const atBottom = thread.scrollHeight - thread.scrollTop - thread.clientHeight < 80;
  change();
  if (atBottom) thread.scrollTop = thread.scrollHeight;
}

function add(chat, node) {
  const thread = threadFor(chat);
  stickToBottom(thread, () => thread.append(node));
  if (chat === open) refreshEmpty();
  return node;
}

function problem(chat, text) {
  if (chat === open && main.classList.contains("empty")) $("hero-note").textContent = text;
  else add(chat, el("p", "note", text));
}

function clearHearing() {
  hearing?.remove();
  hearing = null;
}

async function openChat(chat) {
  if (chat !== open) {
    clearHearing();
    await call.invoke("open", { chat }).catch(() => {});
  }
  open = chat;
  for (const [id, thread] of Object.entries(threads)) thread.hidden = id !== chat;
  threadFor(chat).hidden = false;
  const d = describe(chat);
  $("top-title").textContent = d.title;
  $("top-sub").textContent = d.sub;
  $("top-avatars").replaceChildren(...members(chat).slice(0, 3).map((name) => avatar(name)));
  $("hero-title").textContent = d.hero;
  $("hero-note").textContent = "";
  input.placeholder = d.placeholder;
  markOpen();
  refreshEmpty();
  setVoiceState("Listening");
  input.focus();
}

function setVoiceState(text, who) {
  $("voice-state").textContent = text;
  voicebar.style.setProperty("--wave", who ? color(who) : "var(--text)");
}

function onUpdate(u) {
  const chat = u.chat || open;
  const live = lives[chat];
  switch (u.kind) {
    case "team":
      team = { main: u.main, bots: u.bots, groups: u.groups };
      renderSidebar();
      if (pendingOpen && exists(pendingOpen)) {
        const next = pendingOpen;
        pendingOpen = null;
        openChat(next);
      }
      break;
    case "you": {
      if (chat === open) clearHearing();
      const m = el("div", "msg you");
      m.append(el("div", "bubble", u.text));
      add(chat, m);
      break;
    }
    case "hearing":
      if (chat !== open) break;
      if (!hearing) {
        hearing = el("div", "msg you hearing");
        hearing.append(el("div", "bubble"));
        add(chat, hearing);
      }
      stickToBottom(threadFor(chat), () => (hearing.firstChild.textContent = `${u.text}…`));
      break;
    case "start": {
      const m = el("div", "msg bot live");
      m.setAttribute("aria-busy", "true");
      const head = el("div", "head");
      const meta = el("span", "meta");
      head.append(avatar(u.bot), el("span", "", u.bot), meta);
      const text = el("div", "text");
      m.append(head, text);
      lives[chat] = { bot: u.bot, m, text, meta };
      add(chat, m);
      if (chat === open) setVoiceState(`${u.bot} is thinking…`, u.bot);
      break;
    }
    case "text":
      if (live) stickToBottom(threadFor(chat), () => live.text.append(u.text));
      break;
    case "first_audio":
      if (live) {
        live.meta.textContent = `${u.ms} ms`;
        if (chat === open) setVoiceState(`${live.bot} is speaking`, live.bot);
      }
      break;
    case "done":
    case "cut":
      if (live) {
        live.m.classList.remove("live");
        live.m.removeAttribute("aria-busy");
        if (u.kind === "cut") {
          live.m.classList.add("cut");
          if (u.after_ms != null) live.meta.textContent = `stopped ${u.after_ms} ms after you spoke`;
        } else if (!live.text.textContent.trim()) {
          live.m.remove(); // nothing was said
        }
        delete lives[chat];
      }
      if (chat === open) {
        refreshEmpty();
        setVoiceState("Listening");
      }
      break;
    case "problem":
      problem(chat, u.text);
      break;
  }
}

// Voice and composer

function light(node, ok, yes, no) {
  node.textContent = ok ? yes : no;
  node.classList.toggle("ok", ok);
}

async function setVoice(on) {
  try {
    await call.invoke("voice", { on });
  } catch (e) {
    problem(open, String(e));
    return;
  }
  voiceOn = on;
  voicebar.hidden = !on;
  $("mic").setAttribute("aria-pressed", String(on));
  $("mic").setAttribute("aria-label", on ? "End voice" : "Start voice");
  // Spoken replies shouldn't be read out twice by a screen reader.
  Object.values(threads).forEach((t) => t.setAttribute("aria-live", on ? "off" : "polite"));
  setVoiceState("Listening");
}

function grow() {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
}

$("composer").addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = input.value.trim();
  if (!text) return;
  input.value = "";
  grow();
  try {
    await call.invoke("send", { text });
  } catch (err) {
    problem(open, String(err));
  }
});
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    $("composer").requestSubmit();
  }
});
input.addEventListener("input", grow);
$("mic").addEventListener("click", () => setVoice(!voiceOn));
$("end-voice").addEventListener("click", () => setVoice(false));
$("stop").addEventListener("click", () => call.invoke("stop").catch(() => {}));
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") call.invoke("stop").catch(() => {});
});
$("clear").addEventListener("click", async () => {
  await call.invoke("clear").catch(() => {});
  threadFor(open).replaceChildren();
  delete lives[open];
  clearHearing();
  refreshEmpty();
  input.focus();
});

// Create a bot

async function openBotDialog() {
  const form = $("bot-form");
  form.reset();
  $("speed-out").textContent = "1.0×";
  $("bot-error").textContent = "";
  let voices = FALLBACK_VOICES;
  try {
    const listed = await call.invoke("voices");
    if (listed.length) voices = listed;
  } catch {
    // no ElevenLabs key yet: offer the default voices
  }
  $("voice-select").replaceChildren(...voices.map((v) => Object.assign(el("option"), { value: v.id, textContent: v.name })));
  $("bot-dialog").showModal();
  form.querySelector('[name="name"]').focus();
}

$("bot-form").addEventListener("input", (e) => {
  $("bot-error").textContent = "";
  if (e.target.name === "speed") $("speed-out").textContent = `${Number(e.target.value).toFixed(2).replace(/0$/, "")}×`;
});
$("bot-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  const value = (key) => String(form.get(key) ?? "").trim();
  const newBot = {
    name: value("name"),
    role: value("role"),
    personality: value("personality") || "Friendly and clear",
    voice: value("voice"),
    speed: Number(form.get("speed")),
  };
  const error = !newBot.name
    ? "Give the bot a name."
    : same(newBot.name, "you")
      ? "Pick another name: “You” is you."
      : taken(newBot.name)
        ? `${newBot.name} is already taken.`
        : !newBot.role
          ? "Say what the bot is for."
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

// Start a group chat

function openGroupDialog() {
  $("group-form").reset();
  $("group-error").textContent = "";
  $("group-bots").replaceChildren(
    ...team.bots.map((b) => {
      const label = el("label");
      const box = Object.assign(el("input"), { type: "checkbox", name: "bots", value: b.name });
      label.append(box, avatar(b.name), b.name);
      return label;
    }),
  );
  $("group-dialog").showModal();
}

$("group-form").addEventListener("input", () => ($("group-error").textContent = ""));
$("group-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  const name = String(form.get("name") ?? "").trim();
  const bots = form.getAll("bots").map(String);
  const error = !name
    ? "Give the group a name."
    : taken(name)
      ? `${name} is already taken.`
      : bots.length < 2
        ? "Pick at least two bots."
        : "";
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

$("new-bot").addEventListener("click", openBotDialog);
$("new-group").addEventListener("click", openGroupDialog);
document.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => b.closest("dialog").close()));

// Start up

async function start() {
  let status;
  try {
    status = await call.invoke("status");
  } catch (e) {
    status = { chat: false, voice: false, problem: String(e), main: "", bots: [], groups: [] };
  }
  team = { main: status.main, bots: status.bots, groups: status.groups };
  renderSidebar();
  light($("chat-status"), status.chat, "Chat ready", "Chat needs your Claude key");
  light($("voice-status"), status.voice, "Voice ready", "Voice needs two more keys");
  call.listen("call", (event) => onUpdate(event.payload));
  await openChat("main");
  $("hero-note").textContent = status.problem || "";
}

start();
