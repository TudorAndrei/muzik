use crate::support::StepResult;
use cucumber::gherkin::Step;
use muzik_library::{Fields, SqlValue};
use std::collections::BTreeMap;
use std::path::Path;

fn rows(step: &Step) -> anyhow::Result<&[Vec<String>]> {
    Ok(&step
        .table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the step has no table"))?
        .rows)
}

pub fn pairs(step: &Step) -> anyhow::Result<Vec<(&str, &str)>> {
    rows(step)?
        .iter()
        .map(|row| match row.as_slice() {
            [name, value] => Ok((name.as_str(), value.as_str())),
            _ => anyhow::bail!("each row needs a name and a value"),
        })
        .collect()
}

pub fn records(step: &Step) -> anyhow::Result<Vec<BTreeMap<&str, &str>>> {
    let Some((header, body)) = rows(step)?.split_first() else {
        anyhow::bail!("the table has no header");
    };
    Ok(body
        .iter()
        .map(|row| {
            header
                .iter()
                .map(String::as_str)
                .zip(row.iter().map(String::as_str))
                .collect()
        })
        .collect())
}

pub fn cell<'a>(record: &BTreeMap<&str, &'a str>, column: &str) -> anyhow::Result<&'a str> {
    record
        .get(column)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("the table has no column {column:?}"))
}

pub fn expected(value: &str) -> SqlValue {
    if value == "NULL" {
        return SqlValue::Null;
    }
    if let Ok(integer) = value.parse::<i64>() {
        return SqlValue::Integer(integer);
    }
    if let Ok(real) = value.parse::<f64>() {
        return SqlValue::Real(real);
    }
    SqlValue::Text(value.to_owned())
}

pub fn check_fields(what: &str, fields: &Fields, step: &Step) -> StepResult {
    for (name, value) in pairs(step)? {
        let actual = fields.get(name);
        anyhow::ensure!(
            actual == Some(&expected(value)),
            "{what} has {name} = {actual:?}, not {value:?}"
        );
    }
    Ok(())
}

pub fn check_tags(path: &Path, step: &Step) -> StepResult {
    let tags = muzik_tags::read(path, &[])?;
    for (name, value) in pairs(step)? {
        let actual = tags.fields.get(name).map(String::as_str);
        anyhow::ensure!(
            actual == Some(value),
            "{} has the tag {name} = {actual:?}, not {value:?}",
            path.display()
        );
    }
    Ok(())
}
