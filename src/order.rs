use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    /// Committer timestamp, seconds since the epoch.
    pub time: i64,
    /// First line of the commit message.
    pub subject: String,
    /// Author email and timestamp. A cherry-pick keeps both, which is what lets a copy of the
    /// commit be recognised on another branch.
    pub author: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(10)]
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// Identifies the change across cherry-picks, which give the copy a new hash but keep the
    /// author, the author date and the message.
    pub fn fingerprint(&self) -> String {
        format!("{}\u{1f}{}", self.author, self.subject)
    }
}

/// Orders commits as they appear in the history graph: every commit comes after its
/// parents, and a linear run of commits (a branch segment) is kept together. When
/// several branch segments can go next, the one whose last commit is oldest goes first.
///
/// Parents that are not in `commits` are ignored.
pub fn order(commits: Vec<Commit>) -> Vec<Commit> {
    let n = commits.len();
    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.hash.as_str(), i))
        .collect();
    let parents: Vec<Vec<usize>> = commits
        .iter()
        .map(|c| {
            c.parents
                .iter()
                .filter_map(|p| index.get(p.as_str()).copied())
                .collect()
        })
        .collect();
    let mut children = vec![Vec::new(); n];
    for (i, ps) in parents.iter().enumerate() {
        for &p in ps {
            children[p].push(i);
        }
    }

    // A commit continues its parent's segment when it is the only child of its only parent.
    let continues: Vec<bool> = (0..n)
        .map(|i| parents[i].len() == 1 && children[parents[i][0]].len() == 1)
        .collect();

    // Each segment is keyed by the timestamp of its last commit.
    let mut key = vec![0i64; n];
    for start in (0..n).filter(|&i| !continues[i]) {
        let mut segment = vec![start];
        let mut cur = start;
        while children[cur].len() == 1 && continues[children[cur][0]] {
            cur = children[cur][0];
            segment.push(cur);
        }
        let last_time = commits[cur].time;
        for i in segment {
            key[i] = last_time;
        }
    }

    let mut pending: Vec<usize> = parents.iter().map(Vec::len).collect();
    let mut ready = BinaryHeap::new();
    for i in (0..n).filter(|&i| pending[i] == 0) {
        ready.push(Reverse((key[i], commits[i].time, i)));
    }

    let mut sorted = Vec::with_capacity(n);
    let mut next = None;
    loop {
        let i = match next.take() {
            Some(i) => i,
            None => match ready.pop() {
                Some(Reverse((_, _, i))) => i,
                None => break,
            },
        };
        sorted.push(i);
        for &child in &children[i] {
            pending[child] -= 1;
            if pending[child] == 0 {
                if continues[child] {
                    next = Some(child);
                } else {
                    ready.push(Reverse((key[child], commits[child].time, child)));
                }
            }
        }
    }

    let mut slots: Vec<Option<Commit>> = commits.into_iter().map(Some).collect();
    sorted.into_iter().filter_map(|i| slots[i].take()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str], time: i64) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            time,
            author: String::new(),
            subject: String::new(),
        }
    }

    fn hashes(v: Vec<Commit>) -> Vec<String> {
        v.into_iter().map(|c| c.hash).collect()
    }

    #[test]
    fn linear() {
        let v = vec![c("c", &["b"], 3), c("b", &["a"], 2), c("a", &["base"], 1)];
        assert_eq!(hashes(order(v)), ["a", "b", "c"]);
    }

    #[test]
    fn parallel_branches_keep_together_oldest_tip_first() {
        // base -> a1 (t1) -> a2 (t4)        tip time 4
        // base -> b1 (t2) -> b2 (t3)        tip time 3  => goes first
        let v = vec![
            c("a2", &["a1"], 4),
            c("b2", &["b1"], 3),
            c("b1", &["base"], 2),
            c("a1", &["base"], 1),
        ];
        assert_eq!(hashes(order(v)), ["b1", "b2", "a1", "a2"]);
    }

    #[test]
    fn fork_and_merge() {
        // r -> x (t5) ; r -> y1 (t2) -> y2 (t3) ; m merges x and y2 ; z after m
        let v = vec![
            c("z", &["m"], 7),
            c("m", &["x", "y2"], 6),
            c("x", &["r"], 5),
            c("y2", &["y1"], 3),
            c("y1", &["r"], 2),
            c("r", &[], 1),
        ];
        assert_eq!(hashes(order(v)), ["r", "y1", "y2", "x", "m", "z"]);
    }
}
