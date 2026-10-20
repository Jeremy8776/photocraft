//! Worker-owned selected-model sessions. Drawing code never performs network work.
use super::*;
use model_session::ModelSession;

pub(super) fn worker(jobs: mpsc::Receiver<Job>, results: mpsc::Sender<Reply>, wake: egui::Context) {
    let mut session: Option<ModelSession> = None;
    // The address `session` talks to; a rebinding in the launcher gives the next prepare a new one.
    let mut session_route = None;
    let mut prepared = None;
    for job in jobs {
        let reply = match job {
            Job::Rekey { from, to } => {
                if prepared == Some(from) {
                    prepared = Some(to)
                }
                continue;
            }
            Job::Prepare { key, doc } => {
                prepared = None;
                let result = (|| {
                    let (png, size) = source_png(&doc)?;
                    if png.len() > 16 * 1024 * 1024 {
                        return Err("The Object Selection image exceeds the local model's 16 MiB input limit".to_string());
                    }
                    let route = crate::sam3_route::resolve()?;
                    crate::sam3_server::ensure_running(&route.url, route.model)?;
                    if session_route.as_ref() != Some(&route) {
                        session = Some(ModelSession::new(&route)?);
                        session_route = Some(route);
                    }
                    let client = session.as_mut().ok_or_else(|| "The selection model is unavailable".to_string())?;
                    client.prepare(&png)?;
                    prepared = Some(key);
                    Ok(PreparedModel { size, model: client.model(), canceller: Some(client.canceller()) })
                })();
                Reply::Prepared { key, size: result }
            }
            Job::Preview { key, prompt } => {
                let jpeg = if prepared == Some(key) {
                    session.as_ref().ok_or_else(|| "Selection image not prepared".to_string()).and_then(|s| s.preview(&prompt))
                } else {
                    Err("The Object Selection document changed".into())
                };
                Reply::Previewed { key, jpeg }
            }
            Job::Commit { key, prompt, mode } => {
                let mask = if prepared == Some(key) {
                    session.as_ref().ok_or_else(|| "Selection image not prepared".to_string()).and_then(|s| s.commit(&prompt))
                } else {
                    Err("The Object Selection document changed".into())
                };
                Reply::Committed { key, mode, mask }
            }
            Job::Ground { key, text, mode } => {
                let mask = if prepared == Some(key) {
                    session
                        .as_ref()
                        .ok_or_else(|| "Selection image not prepared".to_string())
                        .and_then(|s| if text.is_empty() { s.ground_subject() } else { text_selection::ground_phrases(&text, |phrase| s.ground(phrase)) })
                } else {
                    Err("The Object Selection document changed".into())
                };
                Reply::Grounded { key, mode, mask }
            }
        };
        if results.send(reply).is_err() {
            break;
        }
        wake.request_repaint();
    }
}
