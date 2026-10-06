use std::path::Path;

use bytesize::ByteSize;
use muzik_core::downloads::scan;

pub fn list(directory: &Path) -> anyhow::Result<()> {
    let items = scan(directory)?;
    if items.is_empty() {
        println!("No downloads found. ({})", directory.display());
        return Ok(());
    }
    println!("Downloaded audio in {}", directory.display());
    let mut total_bytes = 0_u64;
    let mut with_id = 0_usize;
    for item in &items {
        total_bytes = total_bytes.saturating_add(item.size);
        if item.youtube_id.is_some() {
            with_id += 1;
        }
        println!(
            "{}\t{}\t{}",
            item.title,
            item.youtube_id.as_deref().unwrap_or(""),
            ByteSize(item.size)
        );
    }
    println!(
        "Total: {} file(s), {}; {} with a YouTube id.",
        items.len(),
        ByteSize(total_bytes),
        with_id
    );
    Ok(())
}
