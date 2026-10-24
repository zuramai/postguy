use postguy_model::{Collection, Folder, Id, Item};

use crate::error::RunError;

/// Most tree nodes a plan walks, the same cap the engine's request lookup uses.
const MAX_VISITS: usize = 200_000;

/// One request in a run, in execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRequest {
    pub id: Id,
    pub name: String,
    /// Folder names from the collection root down to the request's folder.
    pub path: Vec<String>,
    /// `"http"`, `"grpc"` or `"websocket"`; only HTTP requests are sent by the runner.
    pub protocol: &'static str,
}

/// The requests a run covers, depth-first in tree order. `folder` limits the walk to one
/// folder; `requests` keeps only those ids, in the given order.
pub fn plan(collection: &Collection, folder: Option<&Id>, requests: Option<&[Id]>) -> Result<Vec<PlannedRequest>, RunError> {
    let mut visits = 0usize;
    let (items, base_path): (&[Item], Vec<String>) = match folder {
        Some(id) => {
            let (found, path) = find_folder(&collection.items, id, &mut visits)?;
            (&found.items, path)
        }
        None => (&collection.items, Vec::new()),
    };
    let all = walk(items, base_path, &mut visits)?;
    let Some(wanted) = requests else { return Ok(all) };
    wanted
        .iter()
        .map(|id| all.iter().find(|planned| planned.id == *id).cloned().ok_or_else(|| RunError::NotFound(format!("request `{}`", id.0))))
        .collect()
}

fn find_folder<'a>(items: &'a [Item], id: &Id, visits: &mut usize) -> Result<(&'a Folder, Vec<String>), RunError> {
    let mut stack: Vec<(&'a [Item], Vec<String>)> = vec![(items, Vec::new())];
    while let Some((level, path)) = stack.pop() {
        for item in level {
            *visits = visits.saturating_add(1);
            if *visits > MAX_VISITS {
                return Err(RunError::BadConfig("the collection tree is too large to plan".to_string()));
            }
            if let Item::Folder(folder) = item {
                let mut here = path.clone();
                here.push(folder.name.clone());
                if folder.id == *id {
                    return Ok((folder, here));
                }
                stack.push((&folder.items, here));
            }
        }
    }
    Err(RunError::NotFound(format!("folder `{}`", id.0)))
}

fn walk(items: &[Item], base: Vec<String>, visits: &mut usize) -> Result<Vec<PlannedRequest>, RunError> {
    let mut out = Vec::new();
    let base_len = base.len();
    let mut path = base;
    // Iterators on an explicit stack: depth-first, in item order, with no recursion. Each entry
    // remembers the path length of its own level.
    let mut stack: Vec<(std::slice::Iter<'_, Item>, usize)> = vec![(items.iter(), base_len)];
    while let Some((iter, _)) = stack.last_mut() {
        match iter.next() {
            None => {
                stack.pop();
                path.truncate(stack.last().map_or(base_len, |(_, len)| *len));
            }
            Some(item) => {
                *visits = visits.saturating_add(1);
                if *visits > MAX_VISITS {
                    return Err(RunError::BadConfig("the collection tree is too large to plan".to_string()));
                }
                match item {
                    Item::Folder(folder) => {
                        path.push(folder.name.clone());
                        stack.push((folder.items.iter(), path.len()));
                    }
                    Item::Request(request) => {
                        out.push(PlannedRequest {
                            id: request.id.clone(),
                            name: request.name.clone(),
                            path: path.clone(),
                            protocol: request.kind.protocol(),
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}
