//! `docker ps` / `docker images` / `docker network ls` / `docker
//! volume ls` / `docker stats`.
//!
//! Tabular shape: `CONTAINER ID  IMAGE  COMMAND  CREATED  STATUS
//! PORTS  NAMES`. Auto-detects via the tabular pipeline.

pub fn compact(output: &str) -> String {
    let (_, c, _) = crate::auto::compact(output);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_something() {
        let s = "CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES\nabc123 nginx \"nginx\" 1h ago Up 1h 80/tcp web1\n";
        let out = compact(s);
        assert!(!out.is_empty());
    }
}
