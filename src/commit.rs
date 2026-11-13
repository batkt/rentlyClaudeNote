// Claude ажлын commit хийгдээгүй өөрчлөлт хожим commit хийгдсэн эсэхийг илрүүлнэ.
//
// Commit/push-ийг хүн хийнэ — service зөвхөн уншина (status, log). Ажил дуусахад
// бичсэн `zasvarlasan` ("<repo>: <git status --porcelain мөр>") файлууд:
//   - одоо ч commit хийгдээгүй (status-д байгаа)   → Khuleegdej
//   - цэвэр болж, тэдгээрт хүрсэн commit байгаа     → Commit(…)  — «production-д орсон»
//   - цэвэр болсон ч commit алга (өөрчлөлтийг хаясан) → Butsaasan

use crate::claude::git;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Windows-ийн командын мөрийн уртын хязгаараас (≈32K) хэтрэхгүйн тулд pathspec-ийг хэсэглэнэ
const PATH_KHESEG: usize = 100;
/// Нэг repo-д бичих commit-ийн дээд тоо
const COMMIT_MAX: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitMedeelel {
    pub repo: String,
    pub hash: String,
    /// ISO 8601 (commit хийсэн огноо)
    pub ognoo: String,
    pub zokhiogch: String,
    pub garchig: String,
}

#[derive(Debug, PartialEq)]
pub enum RepoDun {
    Khuleegdej,
    Commit(Vec<CommitMedeelel>),
    Butsaasan,
}

/// "<repo>: <porcelain мөр>" → (repo, файлын зам). `status_murnuud` мөрийг
/// trim хийдэг тул статус нь 1–2 тэмдэгт ("M", "MM", "??", "R ") байж болно.
/// Rename ("R  хуучин -> шинэ") бол шинэ замыг авна. Git non-ASCII/зайтай
/// замыг "…" дотор escape хийж бичдэг тул тайлна.
pub fn zasvar_zadlakh(mur: &str) -> Option<(String, String)> {
    let (repo, status) = mur.split_once(": ")?;
    let (_, zam) = status.split_once(' ')?;
    let zam = zam.trim_start();
    let zam = zam.rsplit(" -> ").next().unwrap_or(zam);
    let zam = if zam.len() >= 2 && zam.starts_with('"') && zam.ends_with('"') {
        quote_tailakh(&zam[1..zam.len() - 1])
    } else {
        zam.to_string()
    };
    (!repo.is_empty() && !zam.is_empty()).then(|| (repo.to_string(), zam))
}

/// Git-ийн C-style quoting (core.quotePath): \\ \" \t \n \ooo (UTF-8 байт)
fn quote_tailakh(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            let octal = |x: u8| (b'0'..=b'7').contains(&x);
            if octal(c) && i + 3 < b.len() && octal(b[i + 2]) && octal(b[i + 3]) {
                let n = (c - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
                out.push(n as u8);
                i += 4;
                continue;
            }
            out.push(match c {
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                other => other,
            });
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `git log --format=%H%x1f%cI%x1f%an%x1f%s` гаралтыг задлана
pub fn log_zadlakh(repo: &str, gar: &str) -> Vec<CommitMedeelel> {
    gar.lines()
        .filter_map(|l| {
            let mut t = l.split('\x1f');
            let hash = t.next()?.trim();
            if hash.is_empty() {
                return None;
            }
            Some(CommitMedeelel {
                repo: repo.to_string(),
                hash: hash.to_string(),
                ognoo: t.next().unwrap_or("").to_string(),
                zokhiogch: t.next().unwrap_or("").to_string(),
                garchig: t.next().unwrap_or("").chars().take(200).collect(),
            })
        })
        .collect()
}

/// Нэг repo-гийн `zamuud` файлуудын төлөвийг тодорхойлно.
/// `suuri` — ажил дуусах үеийн HEAD (байвал `suuri..HEAD`-ээс хайна);
/// эс бөгөөс (хуучин ажил, эсвэл history өөрчлөгдсөн) `since`-ээс хойших commit.
pub async fn repo_shalgakh(
    repo: &str,
    zam: &str,
    zamuud: &[String],
    suuri: Option<&str>,
    since: &str,
) -> Result<RepoDun, String> {
    if zamuud.is_empty() {
        return Ok(RepoDun::Butsaasan);
    }
    for kheseg in zamuud.chunks(PATH_KHESEG) {
        let mut args = vec!["status", "--porcelain", "--"];
        args.extend(kheseg.iter().map(String::as_str));
        if !git(zam, &args, GIT_TIMEOUT).await?.trim().is_empty() {
            return Ok(RepoDun::Khuleegdej);
        }
    }

    let suuri = match suuri {
        Some(h) if !h.is_empty() => {
            let obj = format!("{h}^{{commit}}");
            git(zam, &["cat-file", "-e", &obj], GIT_TIMEOUT).await.ok().map(|_| h)
        }
        _ => None,
    };
    let range = suuri.map(|h| format!("{h}..HEAD"));
    let since_arg = format!("--since={since}");

    let mut commituud: Vec<CommitMedeelel> = Vec::new();
    for kheseg in zamuud.chunks(PATH_KHESEG) {
        let mut args = vec!["log", "--format=%H%x1f%cI%x1f%an%x1f%s"];
        match &range {
            Some(r) => args.push(r),
            None if !since.is_empty() => args.extend(["HEAD", since_arg.as_str()]),
            // Суурь ч, огноо ч алга бол бүх түүхээс хайх нь буруу илрүүлэлт өгнө
            None => return Ok(RepoDun::Khuleegdej),
        }
        args.push("--");
        args.extend(kheseg.iter().map(String::as_str));
        for c in log_zadlakh(repo, &git(zam, &args, GIT_TIMEOUT).await?) {
            if !commituud.iter().any(|x| x.hash == c.hash) {
                commituud.push(c);
            }
        }
    }
    if commituud.is_empty() {
        return Ok(RepoDun::Butsaasan);
    }
    // Хуучнаас нь (эхэлж хийсэн commit эхэнд)
    commituud.sort_by(|a, b| a.ognoo.cmp(&b.ognoo));
    commituud.truncate(COMMIT_MAX);
    Ok(RepoDun::Commit(commituud))
}

/// Commit хийгдээгүй мөр бүрийн ("<repo>: <porcelain мөр>") агуулгын тамга:
/// тухайн замын (untracked хавтас бол доторх) одоо байгаа файлуудын
/// `git hash-object`-ийн жагсаалт. Мөр нь ижил ("M a.js") хэвээр ч агуулга нь
/// өөрчлөгдсөн эсэхийг ялгахад хэрэглэнэ.
pub async fn tamga_avakh(zam: &str, murnuud: &[String]) -> Result<HashMap<String, String>, String> {
    let zamtai: Vec<(&String, String)> = murnuud
        .iter()
        .filter_map(|m| zasvar_zadlakh(m).map(|(_, p)| (m, p)))
        .collect();
    if zamtai.is_empty() {
        return Ok(HashMap::new());
    }
    let paths: Vec<&str> = zamtai.iter().map(|(_, p)| p.as_str()).collect();
    let mut failuud: Vec<String> = Vec::new();
    for kheseg in paths.chunks(PATH_KHESEG) {
        let mut args = vec!["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"];
        args.extend(kheseg);
        for f in git(zam, &args, GIT_TIMEOUT).await?.split('\0') {
            // Устгасан файл index-д байсаар ч диск дээр алга — мөр нь (" D") өөрөө тамга
            if !f.is_empty() && Path::new(zam).join(f).is_file() && !failuud.iter().any(|x| x == f) {
                failuud.push(f.to_string());
            }
        }
    }
    let mut hashuud: HashMap<String, String> = HashMap::new();
    for kheseg in failuud.chunks(PATH_KHESEG) {
        let mut args = vec!["hash-object", "--"];
        args.extend(kheseg.iter().map(String::as_str));
        let gar = git(zam, &args, GIT_TIMEOUT).await?;
        let h: Vec<&str> = gar.lines().collect();
        if h.len() != kheseg.len() {
            return Err(format!("git hash-object: {} файлд {} hash буцаалаа", kheseg.len(), h.len()));
        }
        hashuud.extend(kheseg.iter().cloned().zip(h.into_iter().map(str::to_string)));
    }
    Ok(zamtai
        .into_iter()
        .map(|(m, p)| {
            let mut t: Vec<String> = hashuud
                .iter()
                .filter(|(f, _)| *f == &p || (p.ends_with('/') && f.starts_with(&p)))
                .map(|(f, h)| format!("{f}={h}"))
                .collect();
            t.sort();
            (m.clone(), t.join(";"))
        })
        .collect())
}

/// Claude ажилласны дараах commit хийгдээгүй мөрүүдээс Claude-ийн өөрчлөлтийг
/// ялгана. Ажил эхлэхээс өмнө байсан хэрэглэгчийн өөрчлөлтийг (мөр ба агуулга
/// нь хэвээр) оруулахгүй — эс бөгөөс хэрэглэгч commit хийх хүртэл ажил
/// «шинэчлэлтэд ороогүй» хэвээр үлдэж, дараагийн ээлжид Claude-ийнх гэж андуурагдана.
/// - өмнө нь байгаагүй, эсвэл агуулга нь өөрчлөгдсөн мөр → Claude-ийнх
/// - өмнөх ээлжүүдэд Claude-ийн хийсэн (`umnukh`) мөр → commit хүлээгдэж буй хэвээр
pub fn claude_zasvar(
    umnu: &HashMap<String, String>,
    daraa: &HashMap<String, String>,
    umnukh: &HashSet<String>,
) -> Vec<String> {
    let mut v: Vec<String> = daraa
        .iter()
        .filter(|(m, t)| umnukh.contains(*m) || umnu.get(*m) != Some(*t))
        .map(|(m, _)| m.clone())
        .collect();
    v.sort();
    v
}

/// Олон repo-гийн дүнгээс ажлын нэгдсэн төлөв: аль нэг нь хүлээгдэж байвал
/// хүлээнэ (хэсэгчлэн commit хийсэн ч бүрэн биш); commit байвал khiigdsen;
/// бүгд хаягдсан бол butsaasan.
pub fn negtgekh(dunguud: Vec<RepoDun>) -> Option<(&'static str, Vec<CommitMedeelel>)> {
    let mut commituud = Vec::new();
    for d in dunguud {
        match d {
            RepoDun::Khuleegdej => return None,
            RepoDun::Commit(c) => commituud.extend(c),
            RepoDun::Butsaasan => {}
        }
    }
    Some(if commituud.is_empty() { ("butsaasan", commituud) } else { ("khiigdsen", commituud) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn zasvar_mur_zadlana() {
        let z = |m| zasvar_zadlakh(m);
        assert_eq!(z("zevtabs: M components/Admin.js"), Some(("zevtabs".into(), "components/Admin.js".into())));
        assert_eq!(z("turees: MM a/b.js"), Some(("turees".into(), "a/b.js".into())));
        assert_eq!(
            z("zevtabs: ?? pages/khyanalt/khandaltiinBurtgel/"),
            Some(("zevtabs".into(), "pages/khyanalt/khandaltiinBurtgel/".into()))
        );
        assert_eq!(z("tureesBack: R  old.js -> new.js"), Some(("tureesBack".into(), "new.js".into())));
        assert_eq!(z("turees: M \"a b.js\""), Some(("turees".into(), "a b.js".into())));
        // "Ж.js" — git core.quotePath escape (UTF-8: D0 96)
        assert_eq!(z("turees: ?? \"\\320\\226.js\""), Some(("turees".into(), "Ж.js".into())));
        assert_eq!(z("буруу мөр"), None);
    }

    #[test]
    fn log_zadlana() {
        let gar = "abc\x1f2026-11-09T10:00:00+08:00\x1fbatkt\x1fКомит 1\n\ndef\x1f2026-11-09T11:00:00+08:00\x1fbatkt\x1fx\n";
        let c = log_zadlakh("turees", gar);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].hash, "abc");
        assert_eq!(c[0].garchig, "Комит 1");
        assert_eq!(c[1].ognoo, "2026-11-09T11:00:00+08:00");
    }

    #[test]
    fn negtgel() {
        let c = || CommitMedeelel { repo: "r".into(), hash: "h".into(), ognoo: "".into(), zokhiogch: "".into(), garchig: "".into() };
        assert_eq!(negtgekh(vec![RepoDun::Commit(vec![c()]), RepoDun::Khuleegdej]), None);
        assert_eq!(negtgekh(vec![RepoDun::Butsaasan, RepoDun::Butsaasan]).unwrap().0, "butsaasan");
        let (t, cs) = negtgekh(vec![RepoDun::Butsaasan, RepoDun::Commit(vec![c()])]).unwrap();
        assert_eq!((t, cs.len()), ("khiigdsen", 1));
    }

    fn sh_git(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=batkt", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn repo_uusgekh(ner: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rcn-commit-{ner}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        sh_git(&d, &["init", "-b", "main"]);
        std::fs::write(d.join("a.js"), "1\n").unwrap();
        std::fs::write(d.join("b.js"), "1\n").unwrap();
        sh_git(&d, &["add", "."]);
        sh_git(&d, &["commit", "-m", "init"]);
        d
    }

    fn murnuud(d: &Path) -> Vec<String> {
        let o = std::process::Command::new("git").arg("-C").arg(d).args(["status", "--porcelain"]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).map(|l| format!("r: {}", l.trim())).collect()
    }

    /// Ажил эхлэхээс өмнө байсан хэрэглэгчийн өөрчлөлт zasvarlasan-д орохгүй
    /// (zevbackv2 src/db/index.ts хэргийн давтагдахаас сэргийлнэ)
    #[tokio::test]
    async fn khereglegchiin_uurchlultiig_yalgana() {
        let d = repo_uusgekh("tamga");
        let zam = d.to_str().unwrap();
        // Хэрэглэгчийн өөрчлөлт: a.js, untracked хавтас u/
        std::fs::write(d.join("a.js"), "user\n").unwrap();
        std::fs::create_dir_all(d.join("u")).unwrap();
        std::fs::write(d.join("u/x.js"), "x\n").unwrap();
        let umnu = tamga_avakh(zam, &murnuud(&d)).await.unwrap();

        // Claude: b.js засна, u/ дотор файл нэмнэ; a.js-д хүрэхгүй
        std::fs::write(d.join("b.js"), "claude\n").unwrap();
        std::fs::write(d.join("u/y.js"), "y\n").unwrap();
        let daraa = tamga_avakh(zam, &murnuud(&d)).await.unwrap();
        assert_eq!(claude_zasvar(&umnu, &daraa, &HashSet::new()), vec!["r: ?? u/".to_string(), "r: M b.js".to_string()]);

        // Claude хэрэглэгчийн файлыг мөн засвал (мөр ижил, агуулга өөр) → орно
        std::fs::write(d.join("a.js"), "user+claude\n").unwrap();
        let daraa = tamga_avakh(zam, &murnuud(&d)).await.unwrap();
        assert!(claude_zasvar(&umnu, &daraa, &HashSet::new()).contains(&"r: M a.js".to_string()));

        // Өмнөх ээлжийн Claude-ийн мөр (umnukh) хөндөгдөөгүй ч хүлээгдэж буй хэвээр орно
        let umnukh: HashSet<String> = ["r: M a.js".to_string()].into();
        let daraa = tamga_avakh(zam, &murnuud(&d)).await.unwrap();
        assert!(claude_zasvar(&daraa, &daraa, &umnukh) == vec!["r: M a.js".to_string()]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn khuleegdej_commit_butsaasan() {
        let d = repo_uusgekh("tuluv");
        let zam = d.to_str().unwrap();
        let suuri = sh_git(&d, &["rev-parse", "HEAD"]);
        let zamuud = vec!["a.js".to_string(), "shine/".to_string()];

        // Claude-ийн өөрчлөлт commit хийгдээгүй
        std::fs::write(d.join("a.js"), "2\n").unwrap();
        std::fs::create_dir_all(d.join("shine")).unwrap();
        std::fs::write(d.join("shine/c.js"), "x\n").unwrap();
        assert_eq!(repo_shalgakh("r", zam, &zamuud, Some(&suuri), "").await, Ok(RepoDun::Khuleegdej));

        // Хэсэгчлэн commit хийсэн — shine/ үлдсэн тул хүлээсээр
        sh_git(&d, &["add", "a.js"]);
        sh_git(&d, &["commit", "-m", "a засвар"]);
        assert_eq!(repo_shalgakh("r", zam, &zamuud, Some(&suuri), "").await, Ok(RepoDun::Khuleegdej));

        // Холбоогүй файлын commit тооцогдохгүй
        std::fs::write(d.join("b.js"), "2\n").unwrap();
        sh_git(&d, &["commit", "-am", "b"]);
        sh_git(&d, &["add", "shine"]);
        sh_git(&d, &["commit", "-m", "shine хавтас"]);
        match repo_shalgakh("r", zam, &zamuud, Some(&suuri), "").await.unwrap() {
            RepoDun::Commit(c) => {
                let g: Vec<_> = c.iter().map(|x| x.garchig.as_str()).collect();
                assert_eq!(g.len(), 2);
                assert!(g.contains(&"a засвар") && g.contains(&"shine хавтас") && !g.contains(&"b"));
            }
            x => panic!("{x:?}"),
        }

        // Өөрчлөлтийг хаясан (checkout) → commit алга
        let suuri2 = sh_git(&d, &["rev-parse", "HEAD"]);
        std::fs::write(d.join("a.js"), "3\n").unwrap();
        sh_git(&d, &["checkout", "--", "a.js"]);
        let a = vec!["a.js".to_string()];
        assert_eq!(repo_shalgakh("r", zam, &a, Some(&suuri2), "").await, Ok(RepoDun::Butsaasan));

        // Суурь hash алга (хуучин ажил) — since огноогоор хайна
        assert!(matches!(
            repo_shalgakh("r", zam, &zamuud, Some("0000000000000000000000000000000000000000"), "2000-01-01T00:00:00Z")
                .await
                .unwrap(),
            RepoDun::Commit(_)
        ));
        let _ = std::fs::remove_dir_all(d);
    }
}
