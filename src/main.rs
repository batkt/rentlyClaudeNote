mod claude;

use chrono::Local;
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, time::sleep};
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "RentlyClaudeNote";
const SERVICE_DISPLAY_NAME: &str = "zevTurees Claude ажил гүйцэтгэгч";
/// Явцыг backend руу илгээх, цуцалсан эсэхийг шалгах давтамж
const YAVTS_SECS: u64 = 15;

// ─── Тохиргоо ─────────────────────────────────────────────────────────────────

// Debug derive хийхгүй — token, нууц үг log-д гарахаас сэргийлнэ.
#[derive(Deserialize, Clone)]
struct Config {
    #[serde(default = "default_url")]
    url: String,
    #[serde(default)]
    token: String,
    #[serde(default, rename = "nevtrekhNer")]
    nevtrekh_ner: String,
    #[serde(default, rename = "nuutsUg")]
    nuuts_ug: String,
    #[serde(default, rename = "baiguullagiinId")]
    baiguullagiin_id: String,
    #[serde(default = "default_shalgakh")]
    shalgakh_secs: u64,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    /// Цонхны repo нэр → local зам
    #[serde(default)]
    repos: BTreeMap<String, String>,
    #[serde(default)]
    claude: claude::ClaudeTokhirgoo,
}

fn default_url() -> String {
    "https://turees.zevtabs.mn/api".into()
}
fn default_shalgakh() -> u64 {
    60
}
fn default_timeout() -> u64 {
    20
}

// ─── Log ──────────────────────────────────────────────────────────────────────

type LogFile = Arc<Mutex<Box<dyn Write + Send>>>;

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn open_log(dir: &Path) -> LogFile {
    let log_path = dir.join("rentlyClaudeNote.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("Log файл нээхэд алдаа");
    Arc::new(Mutex::new(Box::new(file)))
}

fn log_write(log: &LogFile, level: &str, msg: &str) {
    let line = format!(
        "[{}] [{}] {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        msg
    );
    print!("{}", line);
    if let Ok(mut f) = log.lock() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// exe-ийн хажуудах config.toml, байхгүй бол одоогийн хавтасных (cargo run).
/// Буцаах хавтас нь log, data хадгалах үндсэн хавтас болно.
fn load_config() -> (Config, PathBuf) {
    let exe_dir = get_exe_dir();
    let candidates = [exe_dir.clone(), PathBuf::from(".")];
    let Some(dir) = candidates.iter().find(|d| d.join("config.toml").is_file()) else {
        eprintln!("config.toml олдсонгүй: {:?}", exe_dir.join("config.toml"));
        std::process::exit(1);
    };
    let config_str = fs::read_to_string(dir.join("config.toml")).unwrap_or_else(|e| {
        eprintln!("config.toml уншихад алдаа: {}", e);
        std::process::exit(1);
    });
    let mut config: Config = toml::from_str(&config_str).unwrap_or_else(|e| {
        eprintln!("config.toml алдаа: {}", e);
        std::process::exit(1);
    });
    config.url = config.url.trim_end_matches('/').to_string();
    (config, dir.clone())
}

fn khost() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

// ─── HTTP ─────────────────────────────────────────────────────────────────────

enum ApiAldaa {
    /// 401 — token дууссан эсвэл буруу
    Erkhgui,
    /// 409 — ажлын төлөв өөрчлөгдсөн (өөр service авсан, цуцалсан)
    Zurchil,
    Busad(String),
}

impl std::fmt::Display for ApiAldaa {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ApiAldaa::Erkhgui => write!(f, "HTTP 401 — эрхгүй"),
            ApiAldaa::Zurchil => write!(f, "HTTP 409 — ажлын төлөв өөрчлөгдсөн"),
            ApiAldaa::Busad(s) => write!(f, "{s}"),
        }
    }
}

struct Api {
    client: Client,
    config: Config,
    token: Option<String>,
}

fn khariu_tovch(body: &str) -> String {
    body.chars().take(300).collect()
}

impl Api {
    fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent("rentlyClaudeNote/0.1")
            .build()
            .expect("HTTP client үүсгэхэд алдаа");
        let token = Some(config.token.trim().to_string()).filter(|t| !t.is_empty());
        Self { client, config, token }
    }

    async fn nevtrey(&mut self) -> Result<(), String> {
        if self.config.nevtrekh_ner.is_empty() || self.config.nuuts_ug.is_empty() {
            return Err("token хүчингүй/байхгүй, config.toml-д nevtrekhNer, nuutsUg тохируулаагүй".into());
        }
        let mut body = json!({
            "nevtrekhNer": self.config.nevtrekh_ner,
            "nuutsUg": self.config.nuuts_ug,
        });
        if !self.config.baiguullagiin_id.is_empty() {
            body["baiguullagiinId"] = json!(self.config.baiguullagiin_id);
        }
        let resp = self
            .client
            .post(format!("{}/ajiltanNevtrey", self.config.url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Нэвтрэх хүсэлт: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Нэвтрэх HTTP {}: {}", status.as_u16(), khariu_tovch(&text)));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("Нэвтрэх хариу: {e}"))?;
        if v.get("multipleOrgs").and_then(Value::as_bool) == Some(true) {
            return Err("Олон байгууллагад бүртгэлтэй — config.toml-д baiguullagiinId тохируулна уу".into());
        }
        let token = v
            .get("token")
            .and_then(Value::as_str)
            .ok_or("Нэвтрэх хариунд token алга")?;
        self.token = Some(token.to_string());
        Ok(())
    }

    async fn khuselt_once(&self, method: &Method, zam: &str, body: Option<&Value>) -> Result<Value, ApiAldaa> {
        let token = self.token.as_deref().unwrap_or_default();
        let mut req = self
            .client
            .request(method.clone(), format!("{}{}", self.config.url, zam))
            .bearer_auth(token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| ApiAldaa::Busad(e.to_string()))?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiAldaa::Erkhgui);
        }
        if status == StatusCode::CONFLICT {
            return Err(ApiAldaa::Zurchil);
        }
        let text = resp.text().await.map_err(|e| ApiAldaa::Busad(e.to_string()))?;
        if status == StatusCode::FORBIDDEN {
            return Err(ApiAldaa::Busad("HTTP 403 — Zevtabs CAdmin1 эрх шаардлагатай".into()));
        }
        if !status.is_success() {
            return Err(ApiAldaa::Busad(format!("HTTP {}: {}", status.as_u16(), khariu_tovch(&text))));
        }
        serde_json::from_str(&text).map_err(|e| ApiAldaa::Busad(format!("JSON: {e}")))
    }

    /// Token байхгүй эсвэл 401 бол нэг удаа дахин нэвтэрч оролдоно.
    async fn khuselt(&mut self, method: Method, zam: &str, body: Option<&Value>) -> Result<Value, ApiAldaa> {
        if self.token.is_none() {
            self.nevtrey().await.map_err(ApiAldaa::Busad)?;
        }
        match self.khuselt_once(&method, zam, body).await {
            Err(ApiAldaa::Erkhgui) => {
                self.token = None;
                self.nevtrey().await.map_err(ApiAldaa::Busad)?;
                self.khuselt_once(&method, zam, body).await
            }
            r => r,
        }
    }

    async fn get(&mut self, zam: &str) -> Result<Value, ApiAldaa> {
        self.khuselt(Method::GET, zam, None).await
    }

    async fn post(&mut self, zam: &str, body: Value) -> Result<Value, ApiAldaa> {
        self.khuselt(Method::POST, zam, Some(&body)).await
    }

    /// /zuragAvya — зураг нь токен шаарддаггүй
    async fn zurag_tatay(&self, id: &str) -> Result<(Vec<u8>, String), String> {
        let resp = self
            .client
            .get(format!(
                "{}/zuragAvya/claudeAjil/{}/{}",
                self.config.url, self.config.baiguullagiin_id, id
            ))
            .send()
            .await
            .map_err(|e| format!("зураг {id}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("зураг {id}: HTTP {}", resp.status().as_u16()));
        }
        let turul = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| format!("зураг {id}: {e}"))?;
        Ok((bytes.to_vec(), zurag_ext(&turul).to_string()))
    }
}

fn zurag_ext(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ─── Ажил гүйцэтгэх ───────────────────────────────────────────────────────────

struct App {
    api: Api,
    config: Config,
    data_dir: PathBuf,
    log: LogFile,
    khost: String,
}

struct Guitsetgel {
    ur: Option<String>,
    zardal_usd: Option<f64>,
    zasvarlasan: Vec<String>,
}

impl App {
    fn new(config: Config, base_dir: &Path, log: LogFile) -> Self {
        let data_dir = base_dir.join("data");
        let _ = fs::create_dir_all(&data_dir);
        Self {
            api: Api::new(config.clone()),
            config,
            data_dir,
            log,
            khost: khost(),
        }
    }

    /// Өмнөх удаа энэ компьютер дээр ажиллаж байгаад service зогссон (khiigdej-д
    /// гацсан) ажлуудыг алдаатай гэж тэмдэглэнэ — цонхноос «Дахин ажиллуулах».
    async fn gatsan_tseverley(&mut self) {
        let jagsaalt = match self.api.get("/claudeAjil?tuluv=khiigdej").await {
            Ok(Value::Array(a)) => a,
            Ok(_) => return,
            Err(e) => {
                log_write(&self.log, "ERROR", &format!("Гацсан ажил шалгах: {e}"));
                return;
            }
        };
        for a in jagsaalt.iter().filter(|a| str_of(a, "khost") == self.khost) {
            let id = str_of(a, "_id");
            let body = json!({
                "tuluv": "aldaa",
                "aldaa": format!("{} дээрх service ажиллаж байхдаа зогссон", self.khost),
            });
            match self.api.post(&format!("/claudeAjil/{id}/ur"), body).await {
                Ok(_) => log_write(&self.log, "WARN", &format!("Гацсан ажил алдаатай болголоо: {id}")),
                Err(e) => log_write(&self.log, "ERROR", &format!("{id}: {e}")),
            }
        }
    }

    /// shine ажлуудыг хуучнаас нь эхэлж нэг нэгээр нь гүйцэтгэнэ.
    async fn shalgay(&mut self) {
        let jagsaalt = match self.api.get("/claudeAjil?tuluv=shine").await {
            Ok(Value::Array(a)) => a,
            Ok(_) => {
                log_write(&self.log, "ERROR", "/claudeAjil: массив биш хариу");
                return;
            }
            Err(e) => {
                log_write(&self.log, "ERROR", &format!("/claudeAjil: {e}"));
                return;
            }
        };
        for a in jagsaalt {
            let id = str_of(&a, "_id").to_string();
            let body = json!({ "khost": self.khost });
            match self.api.post(&format!("/claudeAjil/{id}/avya"), body).await {
                Ok(ajil) => self.guitsetgey(ajil).await,
                // Өөр service авсан, эсвэл хооронд нь ноороглосон/цуцалсан
                Err(ApiAldaa::Zurchil) => continue,
                Err(e) => {
                    log_write(&self.log, "ERROR", &format!("{id} авахад алдаа: {e}"));
                    return;
                }
            }
        }
    }

    async fn guitsetgey(&mut self, ajil: Value) {
        let id = str_of(&ajil, "_id").to_string();
        log_write(&self.log, "INFO", &format!("Ажил эхэллээ: {id} «{}»", str_of(&ajil, "garchig")));
        let (tuluv, g, aldaa) = match self.ajilluulya(&id, &ajil).await {
            Ok(g) => ("duussan", g, None),
            Err((e, g)) => ("aldaa", g, Some(e)),
        };
        match &aldaa {
            None => log_write(
                &self.log,
                "INFO",
                &format!(
                    "Ажил дууслаа: {id} · зардал {} · өөрчлөлт {}",
                    g.zardal_usd.map(|z| format!("${z:.2}")).unwrap_or_else(|| "-".into()),
                    g.zasvarlasan.len()
                ),
            ),
            Some(e) => log_write(&self.log, "ERROR", &format!("Ажил {id}: {e}")),
        }
        let body = json!({
            "tuluv": tuluv,
            "ur": g.ur,
            "aldaa": aldaa,
            "zardalUsd": g.zardal_usd,
            "zasvarlasan": g.zasvarlasan,
        });
        if let Err(e) = self.api.post(&format!("/claudeAjil/{id}/ur"), body).await {
            log_write(&self.log, "ERROR", &format!("{id} үр дүн илгээхэд алдаа: {e}"));
        }
    }

    /// Repo шалгах → зураг татах → Claude ажиллуулах. Алдаа гарсан ч тэр
    /// хүртэлх гүйцэтгэлийг (өөрчлөгдсөн файл) буцаана.
    async fn ajilluulya(&mut self, id: &str, ajil: &Value) -> Result<Guitsetgel, (String, Guitsetgel)> {
        let mut g = Guitsetgel { ur: None, zardal_usd: None, zasvarlasan: Vec::new() };

        // Repo
        let nernuud: Vec<String> = ajil
            .get("repos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if nernuud.is_empty() {
            return Err(("Repo сонгоогүй".into(), g));
        }
        let mut repos = Vec::new();
        for ner in &nernuud {
            let Some(zam) = self.config.repos.get(ner) else {
                return Err((format!("{ner}: config.toml [repos]-д зам тохируулаагүй ({})", self.khost), g));
            };
            if !Path::new(zam).is_dir() {
                return Err((format!("{ner}: {zam} хавтас олдсонгүй"), g));
            }
            repos.push((ner.clone(), zam.clone()));
        }
        let mut bokhir = Vec::new();
        for (ner, zam) in &repos {
            match claude::git_status(zam).await {
                Ok(s) if s.trim().is_empty() => {}
                Ok(_) => bokhir.push(ner.clone()),
                Err(e) => return Err((format!("{ner}: git {e}"), g)),
            }
        }
        if !bokhir.is_empty() && !self.config.claude.bokhir_repo_zuvshuurukh {
            return Err((
                format!(
                    "Commit хийгдээгүй өөрчлөлттэй repo: {} — commit хийгээд «Дахин ажиллуулах» дарна уу",
                    bokhir.join(", ")
                ),
                g,
            ));
        }

        // Зураг
        let ajil_dir = self.data_dir.join(id);
        if let Err(e) = fs::create_dir_all(&ajil_dir) {
            return Err((format!("{}: {e}", ajil_dir.display()), g));
        }
        let mut zurguud = Vec::new();
        for (i, z) in ajil.get("zurguud").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let zid = str_of(z, "id");
            if zid.is_empty() {
                continue;
            }
            let (bytes, ext) = match self.api.zurag_tatay(zid).await {
                Ok(r) => r,
                Err(e) => return Err((e, g)),
            };
            let zam = ajil_dir.join(format!("zurag-{}.{ext}", i + 1));
            if let Err(e) = fs::write(&zam, bytes) {
                return Err((format!("{}: {e}", zam.display()), g));
            }
            zurguud.push((zam.display().to_string().replace('\\', "/"), str_of(z, "tailbar").to_string()));
        }

        let prompt = claude::prompt_beltgey(
            &claude::Ajil {
                garchig: str_of(ajil, "garchig"),
                tailbar: str_of(ajil, "tailbar"),
                repos: &repos,
                zurguud: &zurguud,
            },
            &bokhir,
        );
        let _ = fs::write(ajil_dir.join("prompt.md"), &prompt);

        // Claude — явцыг YAVTS_SECS тутам backend руу илгээж, цуцалсан бол зогсооно
        let zamuud: Vec<String> = repos.iter().map(|(_, z)| z.clone()).collect();
        let tokhirgoo = self.config.claude.clone();
        let stream_path = ajil_dir.join("claude.jsonl");
        let buf: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = self.log.clone();
        let ur = {
            let buf2 = buf.clone();
            let yavts = move |m: &str| {
                log_write(&log, "CLAUDE", m);
                if let Ok(mut b) = buf2.lock() {
                    b.push(m.to_string());
                }
            };
            let claude_fut = claude::ajilluulya(&tokhirgoo, &prompt, &zamuud, &ajil_dir, &stream_path, yavts);
            tokio::pin!(claude_fut);
            let mut interval = tokio::time::interval(Duration::from_secs(YAVTS_SECS));
            interval.tick().await;
            loop {
                tokio::select! {
                    r = &mut claude_fut => break r,
                    _ = interval.tick() => {
                        if self.yavts_ilgeey(id, &buf).await.as_deref() == Some("tsutsalsan") {
                            // claude_fut drop болоход процесс kill_on_drop-оор зогсоно
                            break Err("Цонхноос цуцалсан".into());
                        }
                    }
                }
            }
        };
        self.yavts_ilgeey(id, &buf).await;

        for (ner, zam) in &repos {
            if let Ok(s) = claude::git_status(zam).await {
                g.zasvarlasan
                    .extend(s.lines().filter(|l| !l.trim().is_empty()).map(|l| format!("{ner}: {}", l.trim())));
            }
        }
        match ur {
            Ok(u) => {
                g.ur = Some(u.ur);
                g.zardal_usd = u.zardal_usd;
                Ok(g)
            }
            Err(e) => Err((e, g)),
        }
    }

    /// Хуримтлагдсан явцын мөрийг илгээж, ажлын одоогийн төлөвийг буцаана.
    async fn yavts_ilgeey(&mut self, id: &str, buf: &Arc<Mutex<Vec<String>>>) -> Option<String> {
        let murnuud: Vec<String> = buf.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
        match self.api.post(&format!("/claudeAjil/{id}/yavts"), json!({ "murnuud": murnuud })).await {
            Ok(v) => v.get("tuluv").and_then(Value::as_str).map(str::to_string),
            Err(e) => {
                log_write(&self.log, "WARN", &format!("Явц илгээхэд алдаа: {e}"));
                None
            }
        }
    }
}

async fn run_service_loop(config: Config, base_dir: PathBuf, log: LogFile, mut shutdown_rx: oneshot::Receiver<()>) {
    let mut app = App::new(config, &base_dir, log.clone());
    log_write(
        &log,
        "INFO",
        &format!(
            "rentlyClaudeNote эхэллээ: {} ({} сек тутам), khost {}, repo: {}",
            app.config.url,
            app.config.shalgakh_secs,
            app.khost,
            app.config.repos.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    );
    app.gatsan_tseverley().await;
    loop {
        // Зогсоох үед ажиллаж буй Claude-г зогсооно (kill_on_drop); ажил khiigdej-д
        // үлдэж, дараагийн эхлэлд gatsan_tseverley алдаатай болгоно.
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = app.shalgay() => {}
        }
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = sleep(Duration::from_secs(app.config.shalgakh_secs.max(10))) => {}
        }
    }
    log_write(&log, "INFO", "rentlyClaudeNote зогслоо");
}

/// `rentlyClaudeNote.exe once` — нэг удаа шалгаж, хүлээгдэж буй ажлуудыг гүйцэтгэнэ.
async fn run_once(config: Config, base_dir: PathBuf, log: LogFile) {
    let mut app = App::new(config, &base_dir, log);
    app.gatsan_tseverley().await;
    app.shalgay().await;
}

fn console_run(f: impl FnOnce(Config, PathBuf, LogFile) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) {
    let (config, base_dir) = load_config();
    let log = open_log(&base_dir);
    tokio::runtime::Runtime::new().unwrap().block_on(f(config, base_dir, log));
}

fn console_loop() {
    console_run(|c, d, l| {
        let (_tx, rx) = oneshot::channel::<()>();
        Box::pin(async move {
            let _tx = _tx;
            run_service_loop(c, d, l, rx).await
        })
    });
}

// ─── Windows Service ──────────────────────────────────────────────────────────

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_as_service() {
        let log = open_log(&get_exe_dir());
        log_write(&log, "ERROR", &format!("Service алдаатай зогслоо: {:?}", e));
    }
}

fn run_as_service() -> windows_service::Result<()> {
    let (config, base_dir) = load_config();
    let log = open_log(&base_dir);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let status_handle = service_control_handler::register(SERVICE_NAME, {
        let tx = Arc::clone(&shutdown_tx);
        move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Ok(mut guard) = tx.lock() {
                    if let Some(sender) = guard.take() {
                        let _ = sender.send(());
                    }
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_service_loop(config, base_dir, log.clone(), shutdown_rx));

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

// ─── Service install / uninstall ─────────────────────────────────────────────

fn install_service() -> windows_service::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
    let exe_path = std::env::current_exe().unwrap();

    let _svc = manager.create_service(
        &ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe_path,
            launch_arguments: vec![],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        },
        ServiceAccess::empty(),
    )?;

    println!("✓ Service суулгагдлаа: {}", SERVICE_NAME);
    println!("  Claude-ийн нэвтрэлттэй Windows хэрэглэгчээр ажиллуулах (README-г үз):");
    println!("  sc config {} obj= \".\\<хэрэглэгч>\" password= \"<нууц үг>\"", SERVICE_NAME);
    println!("  Эхлүүлэх: sc start {}", SERVICE_NAME);
    Ok(())
}

fn uninstall_service() -> windows_service::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let svc = manager.open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)?;
    svc.delete()?;
    println!("✓ Service устгагдлаа: {}", SERVICE_NAME);
    Ok(())
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("install") => {
            install_service().unwrap_or_else(|e| eprintln!("Суулгахад алдаа: {:?}", e));
        }
        Some("uninstall") => {
            uninstall_service().unwrap_or_else(|e| eprintln!("Устгахад алдаа: {:?}", e));
        }
        // Console горим — гараар туршихад
        Some("run") => console_loop(),
        Some("once") => console_run(|c, d, l| Box::pin(run_once(c, d, l))),
        Some("help") | Some("--help") | Some("-h") => {
            println!(
                "Ашиглалт:\n  rentlyClaudeNote.exe run        - console горимд ажиллуулна\n  rentlyClaudeNote.exe once       - нэг удаа шалгаж хүлээгдэж буй ажлуудыг гүйцэтгэнэ\n  rentlyClaudeNote.exe install    - Windows Service болгож бүртгэнэ (Administrator)\n  rentlyClaudeNote.exe uninstall  - Windows Service-г устгана (Administrator)"
            );
        }
        _ => {
            // SCM-ээс дуудагдах үед — service dispatcher эхлүүлнэ
            service_dispatcher::start(SERVICE_NAME, ffi_service_main).unwrap_or_else(|_| {
                // SCM байхгүй (гараар ажиллуулсан) бол console горимд орно
                console_loop();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_repos_ba_claude_unshina() {
        let c: Config = toml::from_str(
            r#"
            nevtrekhNer = "CAdmin1"
            [repos]
            tureesBack = "D:/Work/tureesBack"
            parking-v2 = "D:/Work/parking-v2"
            [claude]
            budget_usd = 2.5
            "#,
        )
        .unwrap();
        assert_eq!(c.repos.get("parking-v2").map(String::as_str), Some("D:/Work/parking-v2"));
        assert_eq!(c.claude.budget_usd, 2.5);
        assert_eq!(c.claude.timeout_min, 60);
        assert!(!c.claude.bokhir_repo_zuvshuurukh);
        assert_eq!(c.shalgakh_secs, 60);
    }

    #[test]
    fn claude_khesggui_config() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.claude.exe, "claude");
        assert!(c.repos.is_empty());
    }

    #[test]
    fn zurgiin_ext() {
        assert_eq!(zurag_ext("image/jpeg"), "jpg");
        assert_eq!(zurag_ext("image/png; charset=binary"), "png");
        assert_eq!(zurag_ext(""), "png");
    }
}
