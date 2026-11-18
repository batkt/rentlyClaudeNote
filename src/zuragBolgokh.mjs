// «Claude ажил» бүрийн shalgalt/ хавтаст service бичдэг туслах: HTML-ийг Edge/Chrome-оор
// зураг болгож, Claude үр дүнгээ Read tool-оор нүдээр харж загвартай харьцуулна.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

function khutuchOlokh() {
  const undsen = [process.env["ProgramFiles(x86)"], process.env.ProgramFiles, process.env.LOCALAPPDATA].filter(Boolean);
  const zamuud = undsen.flatMap((d) => [
    path.join(d, "Microsoft/Edge/Application/msedge.exe"),
    path.join(d, "Google/Chrome/Application/chrome.exe"),
  ]);
  const olson = zamuud.find((z) => fs.existsSync(z));
  if (!olson) throw new Error("Edge эсвэл Chrome олдсонгүй: " + zamuud.join(", "));
  return olson;
}

/**
 * @param {string} html бүтэн баримт эсвэл HTML хэсэг
 * @param {string} gargakhZam үүсгэх PNG файлын зам
 * @param {{urgun?: number, undur?: number}} [songolt] пикселээр (анхдагч нь A4 босоо, 96 DPI)
 */
export async function htmlZuragBolgokh(html, gargakhZam, { urgun = 794, undur = 1123 } = {}) {
  const tur = fs.mkdtempSync(path.join(os.tmpdir(), "rcn-zurag-"));
  const gargakh = path.resolve(gargakhZam);
  try {
    const barimt = /<html[\s>]/i.test(html)
      ? html
      : `<!doctype html><html><head><meta charset="utf-8"><style>body{margin:0;background:#fff}</style></head><body>${html}</body></html>`;
    const htmlZam = path.join(tur, "khuudas.html");
    fs.writeFileSync(htmlZam, barimt, "utf8");
    fs.mkdirSync(path.dirname(gargakh), { recursive: true });
    fs.rmSync(gargakh, { force: true });
    execFileSync(
      khutuchOlokh(),
      [
        "--headless=new",
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--hide-scrollbars",
        "--force-device-scale-factor=1",
        `--user-data-dir=${path.join(tur, "profile")}`,
        `--window-size=${urgun},${undur}`,
        `--screenshot=${gargakh}`,
        pathToFileURL(htmlZam).href,
      ],
      { stdio: "ignore", timeout: 60000 },
    );
    if (!fs.existsSync(gargakh)) throw new Error("Хөтөч зураг үүсгэсэнгүй: " + gargakh);
    return gargakh;
  } finally {
    fs.rmSync(tur, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  }
}
