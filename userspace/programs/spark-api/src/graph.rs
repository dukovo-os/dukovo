// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The service graph (ADR 0470): the services a service's file names in
//! `requires`, `wants` and `after`, and what follows from them. Spark
//! asks [`verdict`] whether a service may start now, follows
//! [`pulled_in`] to start what a service needs with it, refuses to start
//! the services [`circles`] finds, and stops services by
//! [`stop_ranks`], so that each stops before what it depends on.
//!
//! `requires` and `wants` imply `after`: a service never starts before
//! one it needs, which is the mistake systemd's separate `Requires=` and
//! `After=` invite.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// How a service's file names another service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// `requires`: started first and ready, or this one does not start;
    /// once it stops for good, this one stops too.
    Requires,
    /// `wants`: started first; this one starts whether or not it does.
    Wants,
    /// `after`: waited for only when it is starting too.
    After,
}

impl Link {
    /// The key a service file names it with.
    pub fn key(self) -> &'static str {
        match self {
            Link::Requires => "requires",
            Link::Wants => "wants",
            Link::After => "after",
        }
    }
}

/// A service, as the graph sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node<'a> {
    /// Its name.
    pub name: &'a str,
    /// Its `order`: lower starts first among services that may start
    /// together, and stops last.
    pub order: u32,
    /// The services its file names, each with how.
    pub links: Vec<(&'a str, Link)>,
}

/// What a service that another names is doing, as that other sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peer<'a> {
    /// Running and ready, or done, as a setup step that ended well is.
    Ready,
    /// Starting, waiting to start, or about to start again.
    Coming,
    /// Neither running nor about to, with the words that say why, to
    /// follow "which": "is stopped", "has no service file".
    Off(&'a str),
}

/// Whether a service may start now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It may.
    Go,
    /// Not yet: the services it waits for, which are coming.
    Wait(Vec<String>),
    /// Not until something changes: why, as a clause after a service's
    /// name, such as "it requires db, which is stopped".
    Refuse(String),
}

/// Whether a service whose file names `links` may start, given what each
/// is doing: every one ready or off, but every required one ready. A
/// required one that is off refuses it at once, whatever the others do.
pub fn verdict<'a>(links: &[(&str, Link)], peer: impl Fn(&str) -> Peer<'a>) -> Verdict {
    let mut coming = Vec::new();
    for &(name, link) in links {
        match peer(name) {
            Peer::Ready => {}
            Peer::Coming => coming.push(name.to_string()),
            Peer::Off(why) if link == Link::Requires => {
                return Verdict::Refuse(format!("it requires {name}, which {why}"));
            }
            Peer::Off(_) => {}
        }
    }
    if coming.is_empty() { Verdict::Go } else { Verdict::Wait(coming) }
}

/// A service that another pulled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pull {
    /// The service pulled in.
    pub name: String,
    /// The service whose file named it first.
    pub by: String,
    /// How that file named it.
    pub link: Link,
}

/// The services `root` pulls in through `requires` and `wants`, at any
/// depth, each once, nearest first. `links` gives the links of the
/// service it is asked about, or None when there is no such service:
/// that one is listed, for the caller to refuse or skip, and pulls in
/// nothing more. `after` pulls in nothing.
pub fn pulled_in(
    root: &str,
    mut links: impl FnMut(&str) -> Option<Vec<(String, Link)>>,
) -> Vec<Pull> {
    let mut pulls: Vec<Pull> = Vec::new();
    let mut next = 0;
    let mut current = root.to_string();
    loop {
        for (name, link) in links(&current).unwrap_or_default() {
            let seen = name == root || pulls.iter().any(|pull| pull.name == name);
            if link != Link::After && !seen {
                pulls.push(Pull { name, by: current.clone(), link });
            }
        }
        let Some(pull) = pulls.get(next) else { return pulls };
        current.clone_from(&pull.name);
        next += 1;
    }
}

/// The services that wait for each other, through links of any kind:
/// each set whose members all reach one another, by name, with the sets
/// in the order of their first member. Names that are no service's are
/// left out; a set of one is no circle.
pub fn circles(nodes: &[Node]) -> Vec<Vec<String>> {
    let index: BTreeMap<&str, usize> =
        nodes.iter().enumerate().map(|(at, node)| (node.name, at)).collect();
    let edges: Vec<Vec<usize>> = nodes
        .iter()
        .map(|node| node.links.iter().filter_map(|(name, _)| index.get(name).copied()).collect())
        .collect();
    let reach: Vec<Vec<bool>> = (0..nodes.len()).map(|from| reaches(&edges, from)).collect();
    let mut placed = vec![false; nodes.len()];
    let mut found = Vec::new();
    for first in 0..nodes.len() {
        if placed[first] {
            continue;
        }
        let members: Vec<usize> = (first..nodes.len())
            .filter(|&other| reach[first][other] && reach[other][first])
            .collect();
        if members.len() < 2 {
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        for &member in &members {
            placed[member] = true;
            names.push(nodes[member].name.to_string());
        }
        names.sort();
        found.push(names);
    }
    found
}

/// Which nodes `from` reaches through one link or more.
fn reaches(edges: &[Vec<usize>], from: usize) -> Vec<bool> {
    let mut reached = vec![false; edges.len()];
    let mut stack: Vec<usize> = edges[from].clone();
    while let Some(at) = stack.pop() {
        if !reached[at] {
            reached[at] = true;
            stack.extend(&edges[at]);
        }
    }
    reached
}

/// Each node's stop rank, by its index in `nodes`: its `order`, raised
/// above the rank of each service it links to, so that shutdown, which
/// stops the highest ranks first, stops a service before what it
/// depends on. Links to names that are no service's, and links that
/// close a circle, which never start, count for nothing.
pub fn stop_ranks(nodes: &[Node]) -> Vec<u32> {
    let index: BTreeMap<&str, usize> =
        nodes.iter().enumerate().map(|(at, node)| (node.name, at)).collect();
    let mut ranks: Vec<Option<u32>> = vec![None; nodes.len()];
    let mut visiting = vec![false; nodes.len()];
    for at in 0..nodes.len() {
        rank(nodes, &index, at, &mut ranks, &mut visiting);
    }
    ranks.into_iter().map(|rank| rank.unwrap_or(0)).collect()
}

/// Node `at`'s stop rank, working out those it links to first. The
/// depth is at most the number of services, a few hundred.
fn rank(
    nodes: &[Node],
    index: &BTreeMap<&str, usize>,
    at: usize,
    ranks: &mut [Option<u32>],
    visiting: &mut [bool],
) -> u32 {
    if let Some(known) = ranks[at] {
        return known;
    }
    visiting[at] = true;
    let mut highest = nodes[at].order;
    for (name, _) in &nodes[at].links {
        let Some(&other) = index.get(name) else { continue };
        if visiting[other] {
            continue;
        }
        let below = rank(nodes, index, other, ranks, visiting);
        highest = highest.max(below.saturating_add(1));
    }
    visiting[at] = false;
    ranks[at] = Some(highest);
    highest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node<'a>(name: &'a str, order: u32, links: &[(&'a str, Link)]) -> Node<'a> {
        Node { name, order, links: links.to_vec() }
    }

    /// db is ready, cache is coming, web is stopped and gone has no file.
    fn peers(name: &str) -> Peer<'static> {
        match name {
            "db" => Peer::Ready,
            "cache" => Peer::Coming,
            "web" => Peer::Off("is stopped"),
            _ => Peer::Off("has no service file"),
        }
    }

    #[test]
    fn a_service_goes_once_what_it_names_is_ready_or_off() {
        let links = [("db", Link::Requires), ("web", Link::Wants), ("gone", Link::After)];
        assert_eq!(verdict(&links, peers), Verdict::Go);
        assert_eq!(verdict(&[], peers), Verdict::Go);
    }

    #[test]
    fn a_service_waits_for_what_is_coming_whatever_the_link() {
        for link in [Link::Requires, Link::Wants, Link::After] {
            let links = [("db", Link::Requires), ("cache", link)];
            assert_eq!(verdict(&links, peers), Verdict::Wait(vec!["cache".into()]));
        }
    }

    #[test]
    fn a_requirement_that_is_off_refuses_even_while_others_are_coming() {
        let links = [("cache", Link::After), ("web", Link::Requires)];
        assert_eq!(
            verdict(&links, peers),
            Verdict::Refuse("it requires web, which is stopped".into())
        );
        assert_eq!(
            verdict(&[("gone", Link::Requires)], peers),
            Verdict::Refuse("it requires gone, which has no service file".into())
        );
    }

    fn links_of(name: &str) -> Option<Vec<(String, Link)>> {
        let links: &[(&str, Link)] = match name {
            "web" => &[("db", Link::Requires), ("cache", Link::Wants), ("netd", Link::After)],
            "db" => &[("disk", Link::Requires), ("web", Link::Wants)],
            "cache" => &[("disk", Link::Wants)],
            "disk" | "netd" => &[],
            _ => return None,
        };
        Some(links.iter().map(|&(name, link)| (name.to_string(), link)).collect())
    }

    #[test]
    fn requires_and_wants_pull_in_at_any_depth_and_after_does_not() {
        let names: Vec<(String, String)> =
            pulled_in("web", links_of).into_iter().map(|pull| (pull.name, pull.by)).collect();
        assert_eq!(
            names,
            [("db", "web"), ("cache", "web"), ("disk", "db")]
                .map(|(name, by)| (name.to_string(), by.to_string()))
        );
    }

    #[test]
    fn a_name_that_is_no_service_is_pulled_in_for_the_caller_to_refuse() {
        let links = |name: &str| match name {
            "web" => Some(vec![("nosuch".to_string(), Link::Requires)]),
            _ => None,
        };
        let pulls = pulled_in("web", links);
        assert_eq!(pulls, [Pull { name: "nosuch".into(), by: "web".into(), link: Link::Requires }]);
    }

    #[test]
    fn circles_are_found_and_a_diamond_is_none() {
        let nodes = [
            node("a", 10, &[("b", Link::After)]),
            node("b", 10, &[("a", Link::Requires)]),
            node("c", 10, &[("d", Link::Wants)]),
            node("d", 10, &[("e", Link::After)]),
            node("e", 10, &[("c", Link::After), ("gone", Link::Requires)]),
            // A diamond: f needs g and h, which both need i.
            node("f", 10, &[("g", Link::Requires), ("h", Link::Requires)]),
            node("g", 10, &[("i", Link::After)]),
            node("h", 10, &[("i", Link::After)]),
            node("i", 10, &[]),
        ];
        assert_eq!(circles(&nodes), [vec!["a", "b"], vec!["c", "d", "e"]]);
    }

    #[test]
    fn a_service_stops_before_what_it_depends_on_whatever_their_order() {
        let nodes = [
            node("web", 60, &[("db", Link::Requires), ("netd", Link::After)]),
            node("db", 70, &[]),
            node("netd", 10, &[]),
            node("lone", 40, &[("gone", Link::After)]),
        ];
        assert_eq!(stop_ranks(&nodes), [71, 70, 10, 40]);
    }

    #[test]
    fn a_circle_gives_ranks_and_does_not_loop() {
        let nodes = [node("a", 10, &[("b", Link::After)]), node("b", 20, &[("a", Link::After)])];
        let ranks = stop_ranks(&nodes);
        assert_eq!(ranks.len(), 2);
        assert!(ranks.iter().all(|&rank| rank >= 10));
    }
}
