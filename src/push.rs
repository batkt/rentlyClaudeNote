// Тохиргоонд (`[claude] avtomat_push`) нэрлэсэн repo-д Claude-ийн ажил амжилттай
// дуусахад service өөрөө commit + push хийнэ. Claude-д commit/push эрх өгөхгүй
// хэвээр — зөвхөн service, зөвхөн Claude-ийн өөрчилсөн файлуудыг.
//
// Хамгаалалт:
//  - Хэрэглэгчийн урьдчилан stage хийсэн өөрчлөлт байвал commit хийхгүй (холихгүй).
//  - `.env*` файлыг хэзээ ч commit хийхгүй.
//  - Force push хийхгүй. Remote урагшилсан бол нэг удаа `pull --rebase --autostash`;
//    зөрчилтэй бол rebase-ийг цуцалж commit-ийг local-д үлдээнэ.

use crate::claude::git;
use crate::commit::zasvar_zadlakh;
use std::time::Duration;

const BOGINO: Duration = Duration::from_secs(60);
const SULJEE: Duration = Duration::from_secs(180);
const PATH_KHESEG: usize = 100;

/// Нэр нь `avtomat_push` жагсаалтад байгаа эсэх (том жижиг үсэг хамаарахгүй)
pub fn idevkhtei(jagsaalt: &[String], ner: &str) -> bool {
    jagsaalt.iter().any(|r| r.trim().eq_ignore_ascii_case(ner))
}

/// Нууц агуулж болзошгүй файл — автоматаар хэзээ ч commit хийхгүй
fn nuuts_fail(zam: &str) -> bool {
    let ner = zam.trim_end_matches('/').rsplit('/').next().unwrap_or(zam);
    ner.starts_with(".env")
}

/// `zasvarlasan` мөрүүдээс ("<repo>: <porcelain мөр>") тухайн repo-гийн файлуудыг
pub fn repo_failuud(zasvarlasan: &[String], ner: &str) -> Vec<String> {
    let mut v: Vec<String> = zasvarlasan
        .iter()
        .filter_map(|m| zasvar_zadlakh(m))
        .filter(|(r, _)| r == ner)
        .map(|(_, f)| f)
        .collect();
    v.sort();
    v.dedup();
    v
}

fn commit_garchig(garchig: &str, id: &str) -> String {
    let g: String = garchig.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect();
    if g.is_empty() {
        format!("Claude ажил {id}")
    } else {
        g
    }
}

/// Claude-ийн өөрчилсөн `failuud`-ыг commit хийж push хийнэ. Амжилттай бол
/// цонхонд харуулах мөрийг, commit хийх зүйлгүй бол None буцаана.
pub async fn commit_push(
    ner: &str,
    zam: &str,
    failuud: &[String],
    garchig: &str,
    id: &str,
) -> Result<Option<String>, String> {
    let (nuuts, failuud): (Vec<&String>, Vec<&String>) = failuud.iter().partition(|f| nuuts_fail(f));
    let nuuts_mur = if nuuts.is_empty() {
        String::new()
    } else {
        format!(" (⚠ commit хийгээгүй: {})", nuuts.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
    };
    if failuud.is_empty() {
        return Ok((!nuuts.is_empty()).then(|| format!("⚠ {ner}: commit хийх файл алга{nuuts_mur}")));
    }

    git(zam, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], BOGINO)
        .await
        .map_err(|_| "upstream branch тохируулаагүй тул commit/push хийсэнгүй".to_string())?;

    // Хэрэглэгч өөрөө stage хийсэн зүйл байвал түүнийг Claude-ийн commit-д холихгүй
    let staged = git(zam, &["diff", "--cached", "--name-only"], BOGINO).await?;
    if !staged.trim().is_empty() {
        return Err(format!(
            "өмнө нь stage хийсэн өөрчлөлт байгаа тул автомат commit хийсэнгүй: {}",
            staged.lines().take(5).collect::<Vec<_>>().join(", ")
        ));
    }

    for kheseg in failuud.chunks(PATH_KHESEG) {
        let mut args = vec!["add", "-A", "--"];
        args.extend(kheseg.iter().map(|s| s.as_str()));
        git(zam, &args, BOGINO).await?;
    }
    if git(zam, &["diff", "--cached", "--name-only"], BOGINO).await?.trim().is_empty() {
        return Ok(None);
    }
    let biy = format!("Claude ажил {id} — rentlyClaudeNote автомат commit");
    git(zam, &["commit", "-m", &commit_garchig(garchig, id), "-m", &biy], BOGINO).await?;
    let hash = git(zam, &["rev-parse", "--short", "HEAD"], BOGINO).await?.trim().to_string();

    if let Err(e1) = git(zam, &["push"], SULJEE).await {
        // Remote урагшилсан байж болно — нэг удаа rebase хийж дахин оролдоно
        if let Err(e2) = git(zam, &["pull", "--rebase", "--autostash"], SULJEE).await {
            let _ = git(zam, &["rebase", "--abort"], BOGINO).await;
            return Err(format!(
                "commit {hash} local-д үлдлээ, push амжилтгүй: {} / rebase: {}",
                crate::claude::tovch(&e1, 200),
                crate::claude::tovch(&e2, 200)
            ));
        }
        git(zam, &["push"], SULJEE)
            .await
            .map_err(|e| format!("commit local-д үлдлээ, push амжилтгүй: {}", crate::claude::tovch(&e, 300)))?;
    }
    let hash = git(zam, &["rev-parse", "--short", "HEAD"], BOGINO).await.map(|h| h.trim().to_string()).unwrap_or(hash);
    Ok(Some(format!("⬆ {ner}: {} файл commit {hash} хийж push хийлээ{nuuts_mur}", failuud.len())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sh(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn tokhiruul(d: &Path) {
        sh(d, &["config", "user.name", "t"]);
        sh(d, &["config", "user.email", "t@t"]);
        sh(d, &["config", "commit.gpgsign", "false"]);
    }

    /// origin (bare) + ажлын clone `b` + origin-д түлхэх өөр clone `a`
    fn orchin(ner: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rcn-push-{ner}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let origin = root.join("origin.git");
        let (a, b) = (root.join("a"), root.join("b"));
        sh(&root, &["init", "--bare", "-b", "main", origin.to_str().unwrap()]);
        sh(&root, &["clone", origin.to_str().unwrap(), a.to_str().unwrap()]);
        tokhiruul(&a);
        sh(&a, &["checkout", "-b", "main"]);
        std::fs::write(a.join("x.js"), "1\n").unwrap();
        std::fs::write(a.join("user.js"), "1\n").unwrap();
        sh(&a, &["add", "."]);
        sh(&a, &["commit", "-m", "init"]);
        sh(&a, &["push", "-u", "origin", "main"]);
        sh(&root, &["clone", origin.to_str().unwrap(), b.to_str().unwrap()]);
        tokhiruul(&b);
        (root, origin, a, b)
    }

    #[test]
    fn jagsaalt_ba_failuud() {
        let j = vec!["irtsBack".to_string(), " irtsWeb ".to_string()];
        assert!(idevkhtei(&j, "irtsweb") && idevkhtei(&j, "irtsBack") && !idevkhtei(&j, "turees"));
        let z = vec!["irtsWeb: M a.js".to_string(), "irtsBack: M b.js".to_string(), "irtsWeb: ?? c/".to_string()];
        assert_eq!(repo_failuud(&z, "irtsWeb"), vec!["a.js".to_string(), "c/".to_string()]);
        assert!(nuuts_fail(".env.local") && nuuts_fail("cfg/.env") && !nuuts_fail("env.js"));
        assert_eq!(commit_garchig("  a\n b ", "1"), "a b");
        assert_eq!(commit_garchig("", "1"), "Claude ажил 1");
    }

    /// Зөвхөн Claude-ийн файл commit/push хийгдэж, хэрэглэгчийн өөрчлөлт, .env үлдэнэ
    #[tokio::test]
    async fn zuvkhun_claude_iin_failyg_push_khiine() {
        let (root, origin, _a, b) = orchin("ok");
        let bs = b.to_str().unwrap();
        std::fs::write(b.join("x.js"), "claude\n").unwrap();
        std::fs::create_dir_all(b.join("shine")).unwrap();
        std::fs::write(b.join("shine/y.js"), "y\n").unwrap();
        std::fs::write(b.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(b.join("user.js"), "user\n").unwrap(); // хэрэглэгчийнх
        let f = vec!["x.js".to_string(), "shine/".to_string(), ".env".to_string()];

        let mur = commit_push("r", bs, &f, "Жилийн календарь", "id1").await.unwrap().unwrap();
        assert!(mur.contains("2 файл") && mur.contains(".env"), "{mur}");
        assert_eq!(sh(&origin, &["log", "-1", "--format=%s", "main"]), "Жилийн календарь");
        let commitd = sh(&origin, &["show", "--name-only", "--format=", "main"]);
        assert!(commitd.contains("x.js") && commitd.contains("shine/y.js"));
        assert!(!commitd.contains("user.js") && !commitd.contains(".env"));
        let st = sh(&b, &["status", "--porcelain"]);
        assert!(st.contains("user.js") && st.contains(".env"), "{st}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Remote урагшилсан бол rebase хийгээд push хийнэ (force хийхгүй)
    #[tokio::test]
    async fn remote_uragshilsan_bol_rebase() {
        let (root, origin, a, b) = orchin("rebase");
        std::fs::write(a.join("other.js"), "o\n").unwrap();
        sh(&a, &["add", "."]);
        sh(&a, &["commit", "-m", "remote"]);
        sh(&a, &["push"]);
        std::fs::write(b.join("x.js"), "claude\n").unwrap();
        std::fs::write(b.join("user.js"), "user\n").unwrap(); // autostash-аар хадгалагдана
        commit_push("r", b.to_str().unwrap(), &["x.js".to_string()], "g", "id2").await.unwrap().unwrap();
        let log = sh(&origin, &["log", "--format=%s", "main"]);
        assert_eq!(log.lines().collect::<Vec<_>>(), vec!["g", "remote", "init"]);
        assert!(sh(&b, &["status", "--porcelain"]).contains("user.js"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Remote-тэй зөрчилтэй бол rebase-ийг цуцалж commit-ийг local-д үлдээнэ
    #[tokio::test]
    async fn zurchiltei_bol_local_d_uldene() {
        let (root, origin, a, b) = orchin("zurchil");
        std::fs::write(a.join("x.js"), "remote\n").unwrap();
        sh(&a, &["commit", "-am", "remote"]);
        sh(&a, &["push"]);
        std::fs::write(b.join("x.js"), "claude\n").unwrap();
        let e = commit_push("r", b.to_str().unwrap(), &["x.js".to_string()], "g", "id3").await.unwrap_err();
        assert!(e.contains("local-д үлдлээ"), "{e}");
        assert_eq!(sh(&origin, &["log", "-1", "--format=%s", "main"]), "remote");
        assert_eq!(sh(&b, &["log", "-1", "--format=%s"]), "g");
        assert!(!b.join(".git/rebase-merge").exists() && !b.join(".git/rebase-apply").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Хэрэглэгч stage хийсэн зүйлтэй бол commit хийхгүй
    #[tokio::test]
    async fn staged_baival_khiikhgui() {
        let (root, origin, _a, b) = orchin("staged");
        std::fs::write(b.join("user.js"), "user\n").unwrap();
        sh(&b, &["add", "user.js"]);
        std::fs::write(b.join("x.js"), "claude\n").unwrap();
        let e = commit_push("r", b.to_str().unwrap(), &["x.js".to_string()], "g", "id4").await.unwrap_err();
        assert!(e.contains("stage"), "{e}");
        assert_eq!(sh(&origin, &["log", "-1", "--format=%s", "main"]), "init");
        let _ = std::fs::remove_dir_all(root);
    }
}
