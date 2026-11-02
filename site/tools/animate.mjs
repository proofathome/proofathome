// Renders the animated share video and GIF: the proof card checks its rows one
// by one, then a full-screen punchline, then it loops back to the first frame.
// Needs ffmpeg on PATH. Run `npm run animate` in this directory.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { Resvg } from "@resvg/resvg-js";

const W = 1200, H = 630, FPS = 30, DURATION = 8;
const SANS = "Segoe UI, sans-serif";
const MONO = "Cascadia Mono, Consolas, monospace";

const ROWS = ["virtio_queue_bounds", "vsock_credit_safe", "blk_sector_in_range", "net_rx_no_overflow"];
const FIRST_START = 0.6, ROW_GAP = 0.75, CHECK_TIME = 0.6;
const ALL_DONE = FIRST_START + (ROWS.length - 1) * ROW_GAP + CHECK_TIME;
const SWITCH = 4.4, FADE = 0.3, LOOP_BACK = 7.4;

const clamp = (x) => Math.min(1, Math.max(0, x));
const ramp = (t, start, len) => clamp((t - start) / len);
const easeOut = (x) => 1 - (1 - x) ** 3;
const easeOutBack = (x) => 1 + 2.7 * (x - 1) ** 3 + 1.7 * (x - 1) ** 2;

const background = `
  <defs>
    <pattern id="dots" width="28" height="28" patternUnits="userSpaceOnUse"><circle cx="14" cy="14" r="1.6" fill="#34362f"/></pattern>
    <linearGradient id="fade" x1="0" x2="1"><stop offset="0.35" stop-color="#fff" stop-opacity="0"/><stop offset="1" stop-color="#fff"/></linearGradient>
    <mask id="dotmask"><rect width="${W}" height="${H}" fill="url(#fade)"/></mask>
    <radialGradient id="glow" cx="0.82" cy="0.2" r="0.55"><stop offset="0" stop-color="#1f6f50" stop-opacity="0.35"/><stop offset="1" stop-color="#1f6f50" stop-opacity="0"/></radialGradient>
    <radialGradient id="glow2" cx="0.5" cy="0.45" r="0.6"><stop offset="0" stop-color="#1f6f50" stop-opacity="0.32"/><stop offset="1" stop-color="#1f6f50" stop-opacity="0"/></radialGradient>
  </defs>
  <rect width="${W}" height="${H}" fill="#141513"/>
  <rect width="${W}" height="${H}" fill="url(#dots)" mask="url(#dotmask)"/>`;

const mark = (x, y, size) => {
  const k = size / 64;
  return `<g transform="translate(${x} ${y}) scale(${k})"><rect width="64" height="64" rx="14" fill="#1f6f50"/><path d="M20 16v32M20 32h24" fill="none" stroke="#fbfaf7" stroke-width="6.5" stroke-linecap="round"/></g>`;
};

function row(name, i, t) {
  const y = 112 + i * 52;
  const start = FIRST_START + i * ROW_GAP;
  const done = start + CHECK_TIME;
  let icon, color;
  if (t < start) {
    icon = `<circle cx="45" cy="${y}" r="13.5" fill="none" stroke="#5d5b55" stroke-width="2.4" stroke-dasharray="5 4"/>`;
    color = "#6f6d66";
  } else if (t < done) {
    const angle = ((t - start) * 540) % 360;
    icon = `<circle cx="45" cy="${y}" r="13.5" fill="none" stroke="#2c2d29" stroke-width="2.4"/>
      <circle cx="45" cy="${y}" r="13.5" fill="none" stroke="#5fc39a" stroke-width="2.6" stroke-linecap="round" stroke-dasharray="22 63" transform="rotate(${angle} 45 ${y})"/>`;
    color = "#a3a199";
  } else {
    const pop = easeOutBack(ramp(t, done, 0.28));
    const draw = easeOut(ramp(t, done + 0.05, 0.25));
    const flash = 1 - ramp(t, done, 0.5);
    icon = `<circle cx="45" cy="${y}" r="${15 + 10 * easeOut(ramp(t, done, 0.5))}" fill="#5fc39a" opacity="${(0.25 * flash).toFixed(3)}"/>
      <g transform="translate(45 ${y}) scale(${(0.5 + 0.5 * pop).toFixed(3)})"><circle r="15" fill="#1d2d26"/>
      <path d="M-6 0.5l4 4 8-9" fill="none" stroke="#5fc39a" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round" stroke-dasharray="18" stroke-dashoffset="${(18 * (1 - draw)).toFixed(2)}"/></g>`;
    color = "#ecebe6";
  }
  return `${icon}<text x="74" y="${y + 7}" fill="${color}">${name}</text>`;
}

function sceneCard(t) {
  const doneShown = ramp(t, ALL_DONE + 0.1, 0.3);
  return `
  <rect width="${W}" height="${H}" fill="url(#glow)"/>
  ${mark(80, 76, 60)}
  <text x="158" y="119" font-family="${SANS}" font-size="38" font-weight="700" letter-spacing="-0.7" fill="#ecebe6">Proof<tspan fill="#5fc39a">@</tspan>home</text>
  <g font-family="${SANS}" font-size="64" font-weight="700" letter-spacing="-1.5" fill="#ecebe6">
    <text x="80" y="290">Help prove</text>
    <text x="80" y="370">the code the</text>
    <text x="80" y="450" fill="#5fc39a">internet runs on.</text>
  </g>
  <text x="80" y="556" font-family="${SANS}" font-size="28" fill="#a3a199">proofathome.org</text>
  <g transform="translate(724 178)">
    <rect width="396" height="300" rx="18" fill="#1a1b18" stroke="#2c2d29" stroke-width="2"/>
    <g font-family="${MONO}" font-size="17">
      <text x="30" y="48" fill="#a3a199" opacity="${(1 - doneShown).toFixed(3)}">checking proofs…</text>
      <text x="30" y="48" fill="#5fc39a" opacity="${doneShown.toFixed(3)}">4 of 4 proofs hold</text>
    </g>
    <path d="M0 72h396" stroke="#2c2d29" stroke-width="2"/>
    <g font-family="${MONO}" font-size="21">${ROWS.map((name, i) => row(name, i, t)).join("")}</g>
  </g>`;
}

function scenePunchline(t) {
  const line1 = easeOut(ramp(t, SWITCH + 0.45, 0.45));
  const line2 = easeOut(ramp(t, SWITCH + 1.0, 0.45));
  const brand = easeOut(ramp(t, SWITCH + 1.6, 0.45));
  return `
  <rect width="${W}" height="${H}" fill="url(#glow2)"/>
  <g font-family="${SANS}" font-weight="700" text-anchor="middle">
    <text x="600" y="${262 + 18 * (1 - line1)}" font-size="60" letter-spacing="-1.2" fill="#ecebe6" opacity="${line1.toFixed(3)}">Don’t take an AI’s word for it.</text>
    <text x="600" y="${384 + 18 * (1 - line2)}" font-size="100" letter-spacing="-2.5" fill="#5fc39a" opacity="${line2.toFixed(3)}">Check the proof.</text>
  </g>
  <g opacity="${brand.toFixed(3)}">
    ${mark(474, 488, 40)}
    <text x="528" y="518" font-family="${SANS}" font-size="28" fill="#a3a199">proofathome.org</text>
  </g>`;
}

function frame(t) {
  // Scenes fade out to the bare background before the next one fades in, and the
  // card scene restarts at the loop-back so the last frame leads into the first.
  const looping = t >= LOOP_BACK;
  const cardOpacity = looping ? ramp(t, DURATION - FADE, FADE) : 1 - ramp(t, SWITCH, FADE);
  const punchOpacity = looping ? 1 - ramp(t, LOOP_BACK, FADE) : ramp(t, SWITCH + FADE, FADE);
  const parts = [];
  if (cardOpacity > 0) parts.push(`<g opacity="${cardOpacity.toFixed(3)}">${sceneCard(looping ? 0 : t)}</g>`);
  if (punchOpacity > 0) parts.push(`<g opacity="${punchOpacity.toFixed(3)}">${scenePunchline(looping ? DURATION : t)}</g>`);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">${background}${parts.join("")}</svg>`;
}

const dir = mkdtempSync(join(tmpdir(), "proofathome-frames-"));
try {
  const total = DURATION * FPS;
  for (let i = 0; i < total; i++) {
    const png = new Resvg(frame(i / FPS), { font: { loadSystemFonts: true } }).render().asPng();
    writeFileSync(join(dir, `f${String(i).padStart(4, "0")}.png`), png);
  }
  const input = ["-y", "-loglevel", "error", "-framerate", String(FPS), "-i", join(dir, "f%04d.png")];
  const out = (path) => new URL(path, import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
  execFileSync("ffmpeg", [...input, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "22", "-preset", "slow",
    "-movflags", "+faststart", out("../public/og.mp4")]);
  execFileSync("ffmpeg", [...input, "-vf",
    "fps=20,scale=800:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a",
    "-loop", "0", out("../public/brand/proofathome.gif")]);
  console.log(`rendered ${total} frames to ../public/og.mp4 and ../public/brand/proofathome.gif`);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
