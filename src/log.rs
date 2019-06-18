//! Provide structures holding all logged information for all tasks.
//! This structure provides intermediate level information.
//! It is a dag of tasks stored in a vector (using indices as pointers).
use crate::fork_join_graph::visualisation;
use crate::raw_events::{RayonEvent, TaskId, TimeStamp};
use crate::storage::Storage;
use crate::svg::write_svg_file;
use itertools::Itertools;
use serde_derive::{Deserialize, Serialize};
use serde_json;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::iter::{repeat, repeat_with};
use std::path::Path;
use std::sync::Arc;

/// The final information produced for log viewers.
/// A 'task' here is not a rayon task but a subpart of one.
/// a simple example with just one call to join will create
/// 4 tasks:
/// - the sequential code executed before the join
/// - the 2 join tasks
/// - the sequential code executed after the join.
/// The set of all tasks form a fork join graph.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskLog {
    /// starting time (in ns after pool creation)
    pub start_time: TimeStamp,
    /// ending time (in ns after pool creation)
    pub end_time: TimeStamp,
    /// id of thread who ran us
    pub thread_id: usize,
    /// indices of children tasks (either when forking or joining)
    pub children: Vec<TaskId>,
}

impl TaskLog {
    /// Return how much time it took to run this task.
    pub fn duration(&self) -> u64 {
        self.end_time - self.start_time
    }
}

/// Logged information.
///
/// This stores tasks information, threads number and run duration.
/// Obtained by `ThreadPool::install`.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunLog {
    /// total number of threads (some might be unused).
    pub threads_number: usize,
    /// fork-join tasks.
    pub tasks_logs: Vec<TaskLog>,
    /// total run time in nanoseconds.
    pub duration: u64,
    /// all strings used for tagging tasks.
    pub tags: Vec<String>,
    /// subgraphs: some parts of the graph can be tagged with a tag and usize
    /// values are: start task, ending task, tag_id, recorded size
    pub subgraphs: Vec<(TaskId, TaskId, usize, usize)>,
}

impl RunLog {
    /// Create a real log from logged events and reset the pool.
    pub(crate) fn new(
        tasks_number: usize,
        _iterators_number: usize,
        tasks_logs: &[Arc<Storage<RayonEvent>>],
        start: TimeStamp,
    ) -> Self {
        let mut seen_tags = HashMap::new(); // associate each take to a usize index
        let mut tags = Vec::new(); // vector containing all tags strings
        let mut tasks_info: Vec<_> = (0..tasks_number)
            .map(|_| TaskLog {
                start_time: 0, // will be filled later
                end_time: 0,
                thread_id: 0,
                children: Vec::new(),
            })
            .collect();

        let threads_number = tasks_logs.len();
        // remember the active task on each thread
        let mut all_active_tasks: Vec<Option<TaskId>> = repeat(None).take(threads_number).collect();
        // remember the active subgraph on each thread (they for a stack)
        let mut all_active_subgraphs: Vec<Vec<usize>> =
            repeat_with(Vec::new).take(threads_number).collect();

        // store all subgraph related informations
        let mut subgraphs = Vec::new();

        for (thread_id, event) in tasks_logs
            .iter()
            .enumerate()
            .map(|(thread_id, thread_log)| thread_log.iter().map(move |log| (thread_id, log)))
            .kmerge_by(|a, b| a.1.time() < b.1.time())
        {
            let active_tasks = &mut all_active_tasks[thread_id];
            let active_subgraphs = &mut all_active_subgraphs[thread_id];
            match *event {
                RayonEvent::Child(c) => {
                    let father = active_tasks.expect("child with no active task as father");
                    tasks_info[father].children.push(c);
                }
                RayonEvent::TaskEnd(time) => {
                    if let Some(task) = active_tasks.take() {
                        tasks_info[task].end_time = time - start;
                    } else {
                        panic!("ending a non started task. are you mixing logged and un-logged computations ?");
                    }
                }
                RayonEvent::TaskStart(task, time) => {
                    tasks_info[task].thread_id = thread_id;
                    tasks_info[task].start_time = time - start;
                    *active_tasks = Some(task);
                }
                RayonEvent::SubgraphStart(work_type, _) | RayonEvent::SubgraphEnd(work_type) => {
                    if let Some(active_task) = active_tasks {
                        let existing_tag = seen_tags.entry(work_type);
                        let tag_index = match existing_tag {
                            Entry::Occupied(o) => *o.get(),
                            Entry::Vacant(v) => {
                                let index = tags.len();
                                v.insert(index);
                                tags.push(work_type.to_string());
                                index
                            }
                        };
                        match *event {
                            RayonEvent::SubgraphStart(_, work_amount) => {
                                active_subgraphs.push(subgraphs.len());
                                subgraphs.push((*active_task, 0, tag_index, work_amount));
                            }
                            RayonEvent::SubgraphEnd(_) => {
                                let graph_index =
                                    active_subgraphs.pop().expect("ending a non started graph");
                                subgraphs[graph_index].1 = *active_task;
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        panic!("tagging a non existing task");
                    }
                }
            }
        }

        let duration = tasks_info.iter().map(|t| t.end_time).max().unwrap()
            - tasks_info.iter().map(|t| t.start_time).min().unwrap();

        RunLog {
            threads_number,
            tasks_logs: tasks_info,
            duration,
            tags,
            subgraphs,
        }
    }

    /// Fuse our tags into given tags hash table.
    pub(crate) fn scan_tags(&self, tags: &mut HashMap<String, usize>) {
        for tag in &self.tags {
            let next_index = tags.len();
            if let Entry::Vacant(v) = tags.entry(tag.clone()) {
                v.insert(next_index);
            }
        }
    }

    /// Re-number tags according to given renumbering.
    /// This is useful for unifying tags accross several logs.
    /// pre-condition: no "holes" in the hashmap's usizes :
    /// they form a contiguous range starting at 0.
    pub(crate) fn update_tags(&mut self, new_tags: &HashMap<String, usize>) {
        let current_tags = &self.tags;
        // adjust tags inside each subraph
        for subgraph in &mut self.subgraphs {
            subgraph.2 = new_tags[&current_tags[subgraph.2]];
        }
        // adjust the tags themselves
        self.tags = new_tags
            .iter()
            .sorted_by_key(|&(_, i)| i)
            .map(|(t, _)| t.clone())
            .collect();
        unimplemented!("please test me")
    }

    /// Load a rayon_logs log file and deserializes it into a `RunLog`.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<RunLog, io::Error> {
        let file = File::open(path).unwrap();
        serde_json::from_reader(file).map_err(|_| ErrorKind::InvalidData.into())
    }

    /// Save an svg file of all logged information.
    pub fn save_svg<P: AsRef<Path>>(&self, path: P) -> Result<(), io::Error> {
        let scene = visualisation(self, None);
        write_svg_file(&scene, path)
    }

    /// Save log file of currently recorded tasks logs.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), io::Error> {
        let file = File::create(path)?;
        serde_json::to_writer(file, &self).expect("failed serializing");
        Ok(())
    }
}
