// 应用图标生成器——「深夜书房的一盏琥珀台灯」（与 Solum Harmony scripts/gen-icon.mjs 同源）。
//
// 构图三层（自下而上）：深夜底色（垂直微渐变的暖炭黑）→ 息壤地平线（下缘的
// 缓弧，弧顶带一条被灯光打亮的琥珀轮辉）→ 琥珀光球（台灯的光本身，核心实色
// + 平方衰减的辉光）。启动器/系统会做圆形或圆角裁切，所以背景满铺、主体收在
// 中心安全圈内。
//
// 为什么是纯 Node 手写光栅而不是引一个图像库：几何只有径向渐变和圆弧两种，
// 4× 超采样 + 盒式降采样就能得到干净的抗锯齿（取舍同鸿蒙仓 H5 加密原语那条）。
//
// 主仓版与鸿蒙版的差异：着色函数一字不改，但要产出 Tauri 桌面全套（png/ico/
// icns）、iOS AppIcon 与 Android adaptive/legacy launcher。着色坐标固定在
// 216 设计系，`shade()` 对界外坐标能自然外推（渐变夹取、圆弧延伸、辉光衰减），
// Android adaptive 前景的 108dp 大画布（可见区只有中央 72dp）就靠这一点。
//
// 用法：node scripts/gen-icon.mjs
import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const DESIGN = 216;       // 设计坐标系边长（沿用鸿蒙版几何参数）
const SS = 4;             // 超采样倍率

// ---- 调色（与桌面壳 dist/index.html 暗色令牌同源） ----
const BG_TOP = [0x1c, 0x1b, 0x22];
const BG_BOTTOM = [0x11, 0x11, 0x15];
const SOIL_TOP = [0x33, 0x27, 0x12];
const SOIL_BOTTOM = [0x1f, 0x18, 0x0c];
const RIM = [0xf0, 0xa8, 0x24];
const ORB_CORE = [0xff, 0xd9, 0x8a];
const ORB_EDGE = [0xf0, 0xa8, 0x24];
const GLOW = [0xf0, 0xa8, 0x24];

// ---- 几何（216 坐标系；圆形裁切安全圈半径约 100）----
const CX = 108;
const ORB_Y = 92;
const ORB_R = 31;
const GLOW_R = 104;
const SOIL_CY = 340;
const SOIL_R = 214;
const RIM_W = 4.2;
const RIM_SPREAD = 78;

const lerp = (a, b, t) => a + (b - a) * t;
const mix = (c1, c2, t) => [lerp(c1[0], c2[0], t), lerp(c1[1], c2[1], t), lerp(c1[2], c2[2], t)];
const clamp01 = (v) => v < 0 ? 0 : v > 1 ? 1 : v;

/** 单点着色，坐标是 216 系的浮点；界外坐标按同一几何自然外推。 */
function shade(x, y) {
  let c = mix(BG_TOP, BG_BOTTOM, clamp01(y / DESIGN));
  const soilD = Math.hypot(x - CX, y - SOIL_CY);
  if (soilD <= SOIL_R) {
    const depth = clamp01((SOIL_R - soilD) / 96);
    c = mix(SOIL_TOP, SOIL_BOTTOM, depth);
    const rimT = clamp01(1 - (SOIL_R - soilD) / RIM_W);
    if (rimT > 0) {
      const falloff = Math.exp(-((x - CX) * (x - CX)) / (RIM_SPREAD * RIM_SPREAD));
      c = mix(c, RIM, rimT * falloff * 0.95);
    }
    const ambient = Math.exp(-((x - CX) * (x - CX)) / (140 * 140)) *
      Math.exp(-(soilD < SOIL_R ? (SOIL_R - soilD) : 0) / 90);
    c = mix(c, RIM, ambient * 0.16);
  }
  const orbD = Math.hypot(x - CX, y - ORB_Y);
  if (orbD < GLOW_R) {
    const g = (1 - orbD / GLOW_R);
    c = mix(c, GLOW, g * g * 0.55);
  }
  if (orbD <= ORB_R) {
    c = mix(ORB_CORE, ORB_EDGE, clamp01(orbD / ORB_R));
  }
  return c;
}

/**
 * 渲染一张 size×size 的 RGBA 图。
 * span：该画布覆盖多少设计单位（216 = 满铺；324 = adaptive 前景的 1.5× 画布，
 * 令中央 72/108 可见区恰好等于完整 216 构图）。
 */
function render(size, span = DESIGN) {
  const offset = (DESIGN - span) / 2;
  const n = size * SS;
  const big = new Float64Array(n * n * 3);
  for (let py = 0; py < n; py++) {
    for (let px = 0; px < n; px++) {
      const c = shade(((px + 0.5) / n) * span + offset, ((py + 0.5) / n) * span + offset);
      const i = (py * n + px) * 3;
      big[i] = c[0];
      big[i + 1] = c[1];
      big[i + 2] = c[2];
    }
  }
  const rgba = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const i = ((y * SS + sy) * n + (x * SS + sx)) * 3;
          r += big[i];
          g += big[i + 1];
          b += big[i + 2];
        }
      }
      const m = SS * SS;
      const o = (y * size + x) * 4;
      rgba[o] = Math.round(r / m);
      rgba[o + 1] = Math.round(g / m);
      rgba[o + 2] = Math.round(b / m);
      rgba[o + 3] = 255;
    }
  }
  return rgba;
}

// ---- PNG 编码（IHDR + IDAT + IEND，filter 0）----
const CRC_TABLE = new Int32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
  CRC_TABLE[n] = c;
}
function crc32(buf) {
  let c = -1;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function encodePng(rgba, size) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

const pngCache = new Map();
function pngAt(size, span = DESIGN) {
  const key = `${size}@${span}`;
  if (!pngCache.has(key)) pngCache.set(key, encodePng(render(size, span), size));
  return pngCache.get(key);
}

// ---- ICO（PNG 压缩条目，Vista+ 均支持）----
function encodeIco(sizes) {
  const entries = sizes.map((s) => ({ size: s, png: pngAt(s) }));
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  let offset = 6 + entries.length * 16;
  const dirs = [];
  for (const e of entries) {
    const d = Buffer.alloc(16);
    d[0] = e.size >= 256 ? 0 : e.size;
    d[1] = e.size >= 256 ? 0 : e.size;
    d.writeUInt16LE(1, 4);   // planes
    d.writeUInt16LE(32, 6);  // bpp
    d.writeUInt32LE(e.png.length, 8);
    d.writeUInt32LE(offset, 12);
    offset += e.png.length;
    dirs.push(d);
  }
  return Buffer.concat([header, ...dirs, ...entries.map((e) => e.png)]);
}

// ---- ICNS（PNG 数据块）----
function encodeIcns(pairs /* [type, size][] */) {
  const chunks = pairs.map(([type, size]) => {
    const png = pngAt(size);
    const head = Buffer.alloc(8);
    head.write(type, 0, 'ascii');
    head.writeUInt32BE(png.length + 8, 4);
    return Buffer.concat([head, png]);
  });
  const total = 8 + chunks.reduce((a, c) => a + c.length, 0);
  const head = Buffer.alloc(8);
  head.write('icns', 0, 'ascii');
  head.writeUInt32BE(total, 4);
  return Buffer.concat([head, ...chunks]);
}

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const iconsDir = join(root, 'crates/solum-app/icons');
const written = [];
function out(rel, buf) {
  const target = join(root, rel);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, buf);
  written.push(`${rel} (${buf.length} bytes)`);
}

// 桌面 PNG 全套（文件名 → 像素边长，对齐既有清单）
const DESKTOP_PNGS = {
  'icon.png': 512, '32x32.png': 32, '64x64.png': 64, '128x128.png': 128, '128x128@2x.png': 256,
  'icon-source.png': 512,
  'StoreLogo.png': 50,
  'Square30x30Logo.png': 30, 'Square44x44Logo.png': 44, 'Square71x71Logo.png': 71,
  'Square89x89Logo.png': 89, 'Square107x107Logo.png': 107, 'Square142x142Logo.png': 142,
  'Square150x150Logo.png': 150, 'Square284x284Logo.png': 284, 'Square310x310Logo.png': 310,
};
for (const [name, size] of Object.entries(DESKTOP_PNGS)) {
  out(`crates/solum-app/icons/${name}`, pngAt(size));
}
out('crates/solum-app/icons/icon.ico', encodeIco([16, 24, 32, 48, 64, 128, 256]));
out('crates/solum-app/icons/icon.icns', encodeIcns([
  ['ic11', 32], ['ic12', 64], ['ic07', 128], ['ic13', 256], ['ic08', 256], ['ic14', 512], ['ic09', 512],
]));

// iOS AppIcon（按现有文件名解析尺寸：AppIcon-WxH@Sx[-1].png → W*S 像素）
const iosDir = join(iconsDir, 'ios');
if (existsSync(iosDir)) {
  for (const name of readdirSync(iosDir)) {
    const m = name.match(/^AppIcon-(\d+(?:\.\d+)?)x\d+(?:\.\d+)?@(\d)x(?:-\d+)?\.png$/);
    if (!m) continue;
    const size = Math.round(parseFloat(m[1]) * parseInt(m[2], 10));
    out(`crates/solum-app/icons/ios/${name}`, pngAt(size));
  }
}

// Android launcher（gen/android 是真实源码）：legacy 方图 + round + adaptive 前景。
// adaptive 前景画布 108dp、可见区中央 72dp → span=324 让完整构图落进可见区，
// 界外部分由 shade() 外推的同一片夜色/辉光补满，被遮罩裁掉也不留硬边。
const ANDROID_RES = 'crates/solum-app/gen/android/app/src/main/res';
const DPIS = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 };
for (const [dpi, scale] of Object.entries(DPIS)) {
  const legacy = Math.round(48 * scale);
  const fg = Math.round(108 * scale);
  out(`${ANDROID_RES}/mipmap-${dpi}/ic_launcher.png`, pngAt(legacy));
  out(`${ANDROID_RES}/mipmap-${dpi}/ic_launcher_round.png`, pngAt(legacy));
  out(`${ANDROID_RES}/mipmap-${dpi}/ic_launcher_foreground.png`, pngAt(fg, 324));
}
// adaptive 背景色对齐夜色底（前景不透明时不可见，但语义上保持一致）
out(`${ANDROID_RES}/values/ic_launcher_background.xml`,
  Buffer.from('<?xml version="1.0" encoding="utf-8"?>\n<resources>\n  <color name="ic_launcher_background">#131316</color>\n</resources>\n'));

for (const line of written) console.log(`written ${line}`);
console.log(`total ${written.length} files`);
