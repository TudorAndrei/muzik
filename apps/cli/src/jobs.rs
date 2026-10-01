use muzik_core::paths::Paths;
use muzik_jobs::CancelRequest;
use muzik_runner::agent::Codex;
use muzik_runner::choices::{self, Choice};
use muzik_runner::{Jobs, Options, Prompt, Runner, job_id, parse_job_id};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

const WORKERS: usize = 5;

static PROMPT: Mutex<()> = Mutex::new(());
static NULL: Value = Value::Null;

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

fn entries(value: &Value, key: &str) -> Vec<Value> {
    field(value, key).as_array().cloned().unwrap_or_default()
}

pub fn open() -> Result<Arc<Jobs>, String> {
    Jobs::open(&Paths::user()).map(Arc::new)
}

pub fn list() -> Result<(), String> {
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

pub fn show(id: &str) -> Result<(), String> {
    let number = parse_job_id(id).ok_or("Enter a job ID such as queue-12.")?;
    let job = open()?
        .get(number)?
        .ok_or_else(|| format!("Job {id} does not exist."))?;
    let question = job.question.ok_or("This job does not wait for a choice.")?;
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

pub fn answer(id: &str, choice: Option<usize>, value: Option<&str>) -> Result<(), String> {
    let number = parse_job_id(id).ok_or("Enter a job ID such as queue-12.")?;
    let jobs = open()?;
    let job = jobs
        .get(number)?
        .ok_or_else(|| format!("Job {id} does not exist."))?;
    let question = job.question.ok_or("This job does not wait for a choice.")?;
    let answer = match (choice, value) {
        (Some(choice), None) => pick(&choices::choices(&question), choice)?,
        (None, Some(value)) => serde_json::from_str(value).unwrap_or(Value::from(value)),
        _ => return Err("Give a choice number or --value, not both.".into()),
    };
    if !jobs.answer(number, &answer)? {
        return Err("This job does not wait for a choice now.".into());
    }
    println!(
        "The answer is saved and {} is back in the queue.",
        job_id(number)
    );
    drain(&jobs)
}

pub fn cancel(id: &str) -> Result<(), String> {
    let number = parse_job_id(id).ok_or("Enter a job ID such as queue-12.")?;
    match open()?.cancel(number)? {
        CancelRequest::Removed => println!("Removed {} from the queue.", job_id(number)),
        CancelRequest::Requested => {
            println!("{} stops at the next safe point.", job_id(number));
        }
        CancelRequest::NotOpen => return Err(format!("{} is not open.", job_id(number))),
    }
    Ok(())
}

pub fn run() -> Result<(), String> {
    drain(&open()?)
}

pub fn drain(jobs: &Arc<Jobs>) -> Result<(), String> {
    let titles = Mutex::new(HashMap::<String, String>::new());
    let Some(runner) = Runner::start(
        Arc::clone(jobs),
        Options {
            workers: WORKERS,
            sink: Arc::new(move |message| report(&titles, &message)),
            ask: Arc::new(ask),
            chooser: Some(Arc::new(Codex)),
            generation: Arc::new(AtomicU64::new(0)),
        },
    )?
    else {
        println!("The desktop app or another muzik process runs the queue. It will do these jobs.");
        return Ok(());
    };
    runner.wait_until_idle(&AtomicBool::new(false));
    let waiting = entries(&jobs.snapshot(), "waiting").len();
    if waiting > 0 {
        println!("{waiting} item(s) wait for a choice. Run `muzik jobs list`.");
    }
    Ok(())
}

fn report(titles: &Mutex<HashMap<String, String>>, message: &Value) {
    let data = field(message, "data");
    let id = text(field(data, "job_id"));
    let Ok(mut titles) = titles.lock() else {
        return;
    };
    match field(message, "event").as_str().unwrap_or("") {
        "job.started" => {
            titles.insert(id.clone(), text(field(data, "title")));
            println!("{id} started: {}", text(field(data, "title")));
        }
        "job.event" => {
            let payload = field(data, "data");
            let line = match field(data, "event").as_str().unwrap_or("") {
                "message" | "log" => text(field(payload, "message")),
                "step_started" => format!("{} started", text(field(payload, "name"))),
                "item_waiting" => format!(
                    "{} waits for a choice: {}",
                    text(field(payload, "title")),
                    choices::title(choices::kind(field(payload, "question")))
                ),
                "agent_decided" => format!(
                    "chose {} ({:.0}%)",
                    text(field(payload, "label")),
                    field(payload, "confidence").as_f64().unwrap_or(0.0) * 100.0
                ),
                "import_finished" => "import finished".into(),
                _ => String::new(),
            };
            if !line.is_empty() {
                println!("  {id}: {line}");
            }
        }
        "job.completed" => println!("{id} finished: {}", title(&titles, &id)),
        "job.cancelled" => println!("{id} cancelled: {}", title(&titles, &id)),
        "job.failed" => println!(
            "{id} failed: {}: {}",
            title(&titles, &id),
            text(field(field(data, "error"), "message"))
        ),
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
    let _prompt = PROMPT.lock().map_err(|_| "The prompt is not available.")?;
    let question = serde_json::json!({"kind":prompt.kind,"payload":prompt.payload});
    println!(
        "{} · {}",
        prompt.title,
        choices::title(choices::kind(&question))
    );
    print_question(&question);
    let options = choices::choices(&question);
    let mut line = String::new();
    loop {
        print!("Choice [1-{}]: ", options.len());
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        line.clear();
        if std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Err("No answer was given.".into());
        }
        match line.trim().parse::<usize>() {
            Ok(choice) => match pick(&options, choice) {
                Ok(value) => return Ok(value),
                Err(message) => println!("{message}"),
            },
            Err(_) => println!("Enter a number."),
        }
    }
}

fn print_question(question: &Value) {
    if let Some(note) = choices::note(question) {
        println!("{note}");
    }
    if let Some(note) = choices::agent_note(field(field(question, "payload"), "agent")) {
        println!("{note}");
    }
    for detail in choices::details(question).iter().take(8) {
        println!("  {detail}");
    }
    let suggested = choices::suggestion(question);
    for (index, option) in choices::choices(question).iter().enumerate() {
        let mut line = format!("{:>3}. {}", index + 1, option.label);
        if !option.meta.is_empty() {
            line.push_str(&format!(" · {}", option.meta));
        }
        if let Some(score) = option.score {
            line.push_str(&format!(" · {score}%"));
        }
        if suggested == Some(index) {
            line.push_str(" (suggested)");
        }
        println!("{line}");
    }
}

fn pick(options: &[Choice], choice: usize) -> Result<Value, String> {
    choice
        .checked_sub(1)
        .and_then(|index| options.get(index))
        .map(|option| option.value.clone())
        .ok_or_else(|| format!("Enter a number from 1 to {}.", options.len()))
}

pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
