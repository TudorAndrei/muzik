use anyhow::{Context, bail};
use muzik_core::chapters::Chapter;
use muzik_core::paths::Paths;
use muzik_core::{DecisionKind, JobEvent};
use muzik_runner::agent::Codex;
use muzik_runner::choices::{self, Choice};
use muzik_runner::{AppEvent, Jobs, Options, Prompt, Runner, job_id, parse_job_id};
use muzik_store::jobs::CancelRequest;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const WORKERS: usize = 5;

static PROMPT: Mutex<()> = Mutex::new(());
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static NULL: Value = Value::Null;

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

fn entries(value: &Value, key: &str) -> Vec<Value> {
    field(value, key).as_array().cloned().unwrap_or_default()
}

pub fn open() -> anyhow::Result<Arc<Jobs>> {
    Ok(Arc::new(Jobs::open(&Paths::user())?))
}

pub fn list() -> anyhow::Result<()> {
    let snapshot = open()?.snapshot();
    let open_jobs = entries(&snapshot, "open");
    let waiting = entries(&snapshot, "waiting");
    if open_jobs.is_empty() && waiting.is_empty() {
        println!("The queue is empty.");
        return Ok(());
    }
    for job in &open_jobs {
        println!(
            "{:<8} {:<10} {:<9} {}",
            text(field(job, "status")),
            text(field(job, "job_id")),
            text(field(job, "kind")),
            text(field(job, "title"))
        );
    }
    for job in &waiting {
        println!(
            "{:<8} {:<10} {:<9} {} · {}",
            "waiting",
            job_id(field(job, "id").as_i64().unwrap_or(0)),
            "item",
            text(field(job, "title")),
            choices::title(choices::kind(job))
        );
    }
    if !waiting.is_empty() {
        println!("Run `muzik jobs show <id>` to see the choices of a waiting item.");
    }
    Ok(())
}

pub fn show(id: &str) -> anyhow::Result<()> {
    let number = parse_job_id(id).context("Enter a job ID such as queue-12.")?;
    let job = open()?
        .get(number)?
        .with_context(|| format!("Job {id} does not exist."))?;
    let question = job
        .question
        .context("This job does not wait for a choice.")?;
    println!(
        "{} · {}",
        job.title,
        choices::title(choices::kind(&question))
    );
    print_question(&question);
    println!(
        "Run `muzik jobs answer {} <number>` to answer.",
        job_id(number)
    );
    Ok(())
}

pub fn answer(id: &str, choice: Option<usize>, value: Option<&str>) -> anyhow::Result<()> {
    let number = parse_job_id(id).context("Enter a job ID such as queue-12.")?;
    let jobs = open()?;
    let job = jobs
        .get(number)?
        .with_context(|| format!("Job {id} does not exist."))?;
    let question = job
        .question
        .context("This job does not wait for a choice.")?;
    let answer = match (choice, value) {
        (Some(choice), None) => pick(&choices::choices(&question), choice)?,
        (None, Some(value)) => serde_json::from_str(value).unwrap_or(Value::from(value)),
        _ => bail!("Give a choice number or --value, not both."),
    };
    if !jobs.answer(number, &answer)? {
        bail!("This job does not wait for a choice now.");
    }
    println!(
        "The answer is saved and {} is back in the queue.",
        job_id(number)
    );
    drain(&jobs)
}

pub fn cancel(id: &str) -> anyhow::Result<()> {
    let number = parse_job_id(id).context("Enter a job ID such as queue-12.")?;
    match open()?.cancel(number)? {
        CancelRequest::Removed => println!("Removed {} from the queue.", job_id(number)),
        CancelRequest::Requested => {
            println!("{} stops at the next safe point.", job_id(number));
        }
        CancelRequest::NotOpen => bail!("{} is not open.", job_id(number)),
    }
    Ok(())
}

pub fn run() -> anyhow::Result<()> {
    drain(&open()?)
}

pub fn drain(jobs: &Arc<Jobs>) -> anyhow::Result<()> {
    let titles = Mutex::new(HashMap::<String, String>::new());
    let failed = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&failed);
    let Some(runner) = Runner::start(
        Arc::clone(jobs),
        Options {
            workers: WORKERS,
            sink: Arc::new(move |event| {
                if matches!(event, AppEvent::JobFailed { .. }) {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                report(&titles, &event);
            }),
            ask: Arc::new(ask),
            chooser: Some(Arc::new(Codex)),
            generation: Arc::new(AtomicU64::new(0)),
        },
    )?
    else {
        println!("The desktop app or another muzik process runs the queue. It will do these jobs.");
        return Ok(());
    };
    let _ = ctrlc::set_handler(|| {
        if !INTERRUPTED.swap(true, Ordering::SeqCst) {
            eprintln!("Stopping the running jobs.");
        }
    });
    runner.wait_until_idle(&INTERRUPTED);
    if INTERRUPTED.load(Ordering::SeqCst) {
        bail!("Interrupted. Queued jobs stay in the queue.");
    }
    let waiting = entries(&jobs.snapshot(), "waiting").len();
    if waiting > 0 {
        println!("{waiting} item(s) wait for a choice. Run `muzik jobs list`.");
    }
    match failed.load(Ordering::SeqCst) {
        0 => Ok(()),
        count => bail!("{count} job(s) failed"),
    }
}

fn report(titles: &Mutex<HashMap<String, String>>, event: &AppEvent) {
    let mut titles = titles.lock();
    match event {
        AppEvent::JobStarted {
            job_id: id, title, ..
        } => {
            titles.insert(id.clone(), title.clone());
            println!("{id} started: {title}");
        }
        AppEvent::JobEvent {
            job_id: id, event, ..
        } => {
            let line = match event {
                JobEvent::Message { message, .. } => message.clone(),
                JobEvent::StepStarted(step) => format!("{step} started"),
                JobEvent::ItemWaiting { title, question } => format!(
                    "{title} waits for a choice: {}",
                    choices::title(choices::kind(question))
                ),
                JobEvent::AgentDecided {
                    label, confidence, ..
                } => format!("chose {label} ({:.0}%)", confidence * 100.0),
                _ => String::new(),
            };
            if !line.is_empty() {
                println!("  {id}: {line}");
            }
        }
        AppEvent::JobCompleted { job_id: id, .. } => {
            println!("{id} finished: {}", title(&titles, id));
        }
        AppEvent::JobCancelled { job_id: id } => {
            println!("{id} cancelled: {}", title(&titles, id));
        }
        AppEvent::JobFailed {
            job_id: id,
            message,
        } => println!("{id} failed: {}: {message}", title(&titles, id)),
        _ => {}
    }
}

fn title(titles: &HashMap<String, String>, id: &str) -> String {
    titles.get(id).cloned().unwrap_or_default()
}

fn ask(prompt: Prompt<'_>) -> Result<Value, String> {
    if !std::io::stdin().is_terminal() {
        return Err("This job needs an answer. Run it in a terminal or in the app.".into());
    }
    let _prompt = PROMPT.lock();
    if prompt.kind == DecisionKind::ChapterEdit {
        return edit_chapters(&prompt.payload);
    }
    let question = serde_json::json!({"kind":prompt.kind,"payload":prompt.payload});
    println!(
        "{} · {}",
        prompt.title,
        choices::title(choices::kind(&question))
    );
    print_notes(&question);
    let index = dialoguer::Select::new()
        .with_prompt("Choice")
        .items(option_lines(&question))
        .default(choices::suggestion(&question).unwrap_or(0))
        .interact_opt()
        .map_err(|error| error.to_string())?
        .ok_or("No answer was given.")?;
    pick(&choices::choices(&question), index.saturating_add(1)).map_err(|error| error.to_string())
}

fn edit_chapters(payload: &Value) -> Result<Value, String> {
    let chapters = entries(payload, "chapters")
        .iter()
        .map(|record| {
            Some(Chapter {
                index: u32::try_from(field(record, "index").as_u64()?).ok()?,
                start: field(record, "start").as_i64()?,
                end: field(record, "end").as_i64(),
                title: field(record, "title").as_str()?.to_owned(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("The chapters to edit are not valid.")?;
    let edited = crate::split::edit_chapters(&chapters).map_err(|error| crate::describe(&error))?;
    let chosen = if edited.is_empty() { chapters } else { edited };
    Ok(Value::Array(
        chosen
            .iter()
            .map(|chapter| {
                serde_json::json!({"index":chapter.index,"start":chapter.start,"end":chapter.end,"title":chapter.title})
            })
            .collect(),
    ))
}

fn print_question(question: &Value) {
    print_notes(question);
    for (index, line) in option_lines(question).iter().enumerate() {
        println!("{:>3}. {line}", index.saturating_add(1));
    }
}

fn print_notes(question: &Value) {
    if let Some(note) = choices::note(question) {
        println!("{note}");
    }
    if let Some(note) = choices::agent_note(field(field(question, "payload"), "agent")) {
        println!("{note}");
    }
    for detail in choices::details(question).iter().take(8) {
        println!("  {detail}");
    }
}

fn option_lines(question: &Value) -> Vec<String> {
    let suggested = choices::suggestion(question);
    choices::choices(question)
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let mut line = option.label.clone();
            if !option.meta.is_empty() {
                line.push_str(&format!(" · {}", option.meta));
            }
            if let Some(score) = option.score {
                line.push_str(&format!(" · {score}%"));
            }
            if suggested == Some(index) {
                line.push_str(" (suggested)");
            }
            line
        })
        .collect()
}

fn pick(options: &[Choice], choice: usize) -> anyhow::Result<Value> {
    choice
        .checked_sub(1)
        .and_then(|index| options.get(index))
        .map(|option| option.value.clone())
        .with_context(|| format!("Enter a number from 1 to {}.", options.len()))
}

pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
