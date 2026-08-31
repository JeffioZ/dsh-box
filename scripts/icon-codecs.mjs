// 图标编解码（纯格式处理，无业务逻辑）：
// - encodeIco：RGBA 尺寸表 → ICO（小尺寸 32bpp BMP 条目，大尺寸 PNG 条目）
// - encodeIcns：PNG 尺寸表 → ICNS（内嵌 PNG 条目，macOS 现代格式）
// - encodePng / decodePng：最小 PNG 编解码（8bit RGBA/RGB，filter 0-4）
import zlib from 'node:zlib';

// 极简 CRC32
const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// 纯 JS 最小 PNG 编码器（RGBA → PNG，用于最近邻放大预览）
export function encodePng(width, height, rgba) {
  const raw = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) {
    raw[y * (width * 4 + 1)] = 0; // filter none
    rgba.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body) >>> 0);
    return Buffer.concat([len, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const idat = zlib.deflateSync(raw);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', idat),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// 解码 PNG 到 RGBA（仅支持 8bit RGBA/RGB PNG）
export function decodePng(buf) {
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  let pos = 8;
  const readChunk = () => {
    const len = view.getUint32(pos);
    const type = buf.toString('ascii', pos + 4, pos + 8);
    const data = buf.subarray(pos + 8, pos + 8 + len);
    pos += 12 + len;
    return { type, data };
  };
  let width = 0, height = 0, idat = [];
  let bitDepth = 8;
  let colorType = 0;
  let interlace = 0;
  while (pos < buf.length) {
    const { type, data } = readChunk();
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      interlace = data[12];
    } else if (type === 'IDAT') {
      idat.push(data);
    } else if (type === 'IEND') break;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  // 灰度/调色板等非 RGB/RGBA 布局与 16bit 位深会被下面的逐字节逻辑静默错解，显式拒绝
  if (bitDepth !== 8 || (colorType !== 2 && colorType !== 6)) {
    throw new Error(`decodePng: 不支持的 bitDepth=${bitDepth}/colorType=${colorType}（仅支持 8bit RGB/RGBA）`);
  }
  // Adam7 隔行扫描的扫描线布局不同，逐行重构造辑同样会静默错解，显式拒绝
  if (interlace !== 0) {
    throw new Error(`decodePng: 不支持 interlace=${interlace}（仅支持非隔行 PNG）`);
  }
  const bpp = colorType === 6 ? 4 : 3;
  const stride = width * bpp;
  const out = Buffer.alloc(width * height * 4);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    // 规范只允许 0-4；未知值若按 filter 0 处理会静默错解，显式拒绝
    if (filter > 4) throw new Error(`decodePng: 不支持的 filter=${filter}（第 ${y} 行，PNG 规范仅 0-4）`);
    const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const recon = Buffer.alloc(stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? recon[x - bpp] : 0;
      const b = prev[x];
      const c = x >= bpp ? prev[x - bpp] : 0;
      let v = row[x];
      if (filter === 1) v = (v + a) & 0xff;
      else if (filter === 2) v = (v + b) & 0xff;
      else if (filter === 3) v = (v + ((a + b) >> 1)) & 0xff;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
        const pr = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        v = (v + pr) & 0xff;
      }
      recon[x] = v;
    }
    for (let x = 0; x < width; x++) {
      const i = x * bpp;
      out[y * width * 4 + x * 4] = recon[i];
      out[y * width * 4 + x * 4 + 1] = recon[i + 1];
      out[y * width * 4 + x * 4 + 2] = recon[i + 2];
      out[y * width * 4 + x * 4 + 3] = colorType === 6 ? recon[i + 3] : 255;
    }
    prev = recon;
  }
  return { width, height, rgba: out };
}

// ICNS 内嵌条目（类型码, 像素尺寸）
export const icnsTypes = [
  ['icp4', 16], ['icp5', 32], ['icp6', 64],
  ['ic07', 128], ['ic08', 256], ['ic09', 512], ['ic10', 1024],
];

// ICNS：容器内直接嵌各尺寸 PNG（现代 macOS 全支持）。
export function encodeIcns(pngBySize) {
  const entries = icnsTypes.map(([type, size]) => {
    const data = pngBySize[size];
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length + 8); // 含 8 字节 type+length 头
    return Buffer.concat([Buffer.from(type, 'ascii'), len, data]);
  });
  const body = Buffer.concat(entries);
  const header = Buffer.alloc(8);
  header.write('icns', 0, 'ascii');
  header.writeUInt32BE(body.length + 8, 4);
  return Buffer.concat([header, body]);
}

// ICO：小尺寸（≤64）用 32bpp BGRA BMP 条目（兼容性最好）；
// 大尺寸（128/256）用 PNG 压缩条目（Vista+ 标准做法，体积小且兼容 256 显示）。
export function encodeIco(rgbaBySize) {
  const entries = [];
  const images = [];
  let offset = 6 + rgbaBySize.length * 16;
  for (const { size, rgba } of rgbaBySize) {
    let image;
    if (size >= 128) {
      // PNG 条目：透明由 PNG alpha 承担
      image = encodePng(size, size, rgba);
    } else {
      const info = Buffer.alloc(40);
      info.writeInt32LE(40, 0); // biSize
      info.writeInt32LE(size, 4); // biWidth
      info.writeInt32LE(size * 2, 8); // biHeight（XOR + AND）
      info.writeInt16LE(1, 12); // biPlanes
      info.writeInt16LE(32, 14); // biBitCount
      info.writeUInt32LE(0, 16); // biCompression BI_RGB
      info.writeUInt32LE(size * size * 4, 20); // biSizeImage（XOR）
      // 底部朝上 BGRA
      const xor = Buffer.alloc(size * size * 4);
      for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
          const src = (y * size + x) * 4;
          const dst = ((size - 1 - y) * size + x) * 4;
          xor[dst] = rgba[src + 2]; // B
          xor[dst + 1] = rgba[src + 1]; // G
          xor[dst + 2] = rgba[src]; // R
          xor[dst + 3] = rgba[src + 3]; // A
        }
      }
      // AND 掩码：行按 32bit 对齐，全 0（透明度由 alpha 通道承担）
      const andStride = Math.ceil(size / 32) * 4;
      const and = Buffer.alloc(size * andStride);
      image = Buffer.concat([info, xor, and]);
    }
    const entry = Buffer.alloc(16);
    entry[0] = size >= 256 ? 0 : size;
    entry[1] = size >= 256 ? 0 : size;
    entry.writeUInt16LE(1, 4); // planes
    entry.writeUInt16LE(32, 6); // bpp
    entry.writeUInt32LE(image.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += image.length;
    entries.push(entry);
    images.push(image);
  }
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(rgbaBySize.length, 4);
  return Buffer.concat([header, ...entries, ...images]);
}
