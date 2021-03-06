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
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::iter::successors;
use std::iter::{repeat, repeat_with};
use std::path::Path;
use std::{cmp::Ordering, sync::Arc};

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
    pub fn duration(&self) -> TimeStamp {
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
    pub duration: TimeStamp,
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
                RayonEvent::SubgraphStart(work_type) | RayonEvent::SubgraphEnd(work_type, _) => {
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
                            RayonEvent::SubgraphStart(_) => {
                                active_subgraphs.push(subgraphs.len());
                                subgraphs.push((*active_task, 0, tag_index, 0));
                            }
                            RayonEvent::SubgraphEnd(_, work_amount) => {
                                let graph_index =
                                    active_subgraphs.pop().expect("ending a non started graph");
                                subgraphs[graph_index].1 = *active_task;
                                subgraphs[graph_index].3 = work_amount;
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

    /// Iterate on all tasks between (including) start and end.
    /// pre-condition: start is ancestor of end and all descendants of start
    /// are either ancestors of end or descendants of end.
    fn tasks_between<'a>(
        &'a self,
        start: TaskId,
        end: TaskId,
    ) -> impl Iterator<Item = TaskId> + 'a {
        let mut stack = Vec::new();
        let mut seen = HashSet::new();
        successors(Some(start), move |&n| {
            seen.insert(n);
            if n != end {
                stack.extend(
                    self.tasks_logs[n]
                        .children
                        .iter()
                        .filter(|&c| !seen.contains(c))
                        .cloned(),
                )
            }
            stack.pop()
        })
    }

    /// This returns a HashMap that maps each tag to the number of tasks it has created in the run.
    pub(crate) fn count_tasks(&self) -> HashMap<String, usize> {
        let mut task_profile = HashMap::new();
        for (start_task, end_task, tag_id, _) in self.subgraphs.iter() {
            let current_count = self.tasks_between(*start_task, *end_task).count();
            let old_count = task_profile.entry(self.tags[*tag_id].clone()).or_insert(0);
            *old_count += current_count;
        }
        task_profile
    }

    /// Compute for each task/tag combination the label and opacity of the task.
    /// We return a HashMap indexed by TaskId containing a HashMap indexed by Tag containing
    /// a label and an opacity.
    /// pre-condition: nested subgraphs of same tags will work if subgraphs are ordered topologically
    /// (they should be).
    pub(crate) fn compute_tasks_information(
        &self,
    ) -> HashMap<TaskId, HashMap<String, (String, f64)>> {
        // we start by computing speeds for each subgraph
        // we associate to each tag a hashmap (keys are subgraph) of all raw speeds (and subgraph duration).
        let mut tags_information = HashMap::new();
        for (subgraph_index, (start_task, end_task, tag_id, size)) in
            self.subgraphs.iter().enumerate()
        {
            let total_duration: TimeStamp = self
                .tasks_between(*start_task, *end_task)
                .map(|t| self.tasks_logs[t].duration())
                .sum();
            let speed = *size as f64 / (total_duration as f64);
            tags_information
                .entry(tag_id)
                .or_insert_with(HashMap::new)
                .insert(subgraph_index, (speed, total_duration));
        }
        // normalize speeds
        for informations in tags_information.values_mut() {
            let best_speed = informations
                .values()
                .map(|i| i.0)
                .max_by(|s1, s2| s1.partial_cmp(s2).unwrap())
                .unwrap();
            for information in informations.values_mut() {
                information.0 /= best_speed
            }
        }
        // ok, we are now ready to compute tasks information
        let mut tasks_information = HashMap::new();
        for (subgraph_index, (start_task, end_task, tag_id, size)) in
            self.subgraphs.iter().enumerate()
        {
            for task in self.tasks_between(*start_task, *end_task) {
                let duration = self.tasks_logs[task].duration();
                let (speed, total_duration) = tags_information[&tag_id][&subgraph_index];
                let r = duration as f64 / (total_duration as f64);
                let size_part = (*size as f64 * r).round() as usize; // the task's extrapolated part of the subgraph
                tasks_information
                    .entry(task)
                    .or_insert_with(HashMap::new) // insert because of subgraphs topological ordering
                    // this way we get the innermost recursive subgraph
                    .insert(
                        self.tags[*tag_id].clone(),
                        (
                            format!(
                                "task: {}\ncounted: {}/{}\nduration: {} (micro sec)\nspeed: {}\nthread: {}",
                                task,
                                size_part,
                                size,
                                duration / 1000,
                                speed,
                                self.tasks_logs[task].thread_id
                            ),
                            0.4 + speed * 0.6,
                        ),
                    );
            }
        }
        // final step, add information for no tags
        for (task_id, task) in self.tasks_logs.iter().enumerate() {
            let duration = task.duration();
            tasks_information
                .entry(task_id)
                .or_insert_with(HashMap::new)
                .insert(
                    "_NO_TAGS_".to_string(),
                    (
                        format!(
                            "task: {}\nduration: {} (micro sec)\nthread: {}",
                            task_id,
                            duration / 1000,
                            task.thread_id
                        ),
                        1.0,
                    ),
                );
        }
        tasks_information
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
        // unimplemented!("please test me")
    }

    /// Compute for each tag, the (total work, total duration, normalised speed).
    pub fn stats(&self) -> HashMap<String, (usize, u64, f64)> {
        let mut hash = HashMap::new();
        self.subgraphs
            .iter()
            .for_each(|&(start_task, end_task, tag_id, work)| {
                let subgraph_duration = self
                    .tasks_between(start_task, end_task)
                    .map(|t| self.tasks_logs[t].duration())
                    .sum::<u64>();
                let stat = hash.entry(self.tags[tag_id].clone()).or_insert((0, 0, 0.0));
                stat.0 += work;
                stat.1 += subgraph_duration;
                stat.2 = stat.0 as f64 / stat.1 as f64;
            });
        let max_speed = hash
            .values()
            .map(|(_, _, s)| *s)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Less))
            .unwrap_or(1.0);
        //Normalise the speeds across tags
        hash.values_mut().for_each(|(_, _, speed)| {
            *speed = *speed / max_speed;
        });
        hash
    }

    /// Load a rayon_logs log file and deserializes it into a `RunLog`.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<RunLog, io::Error> {
        let file = File::open(path).unwrap();
        serde_json::from_reader(file).map_err(|_| ErrorKind::InvalidData.into())
    }

    /// Save an svg file of all logged information.
    pub fn save_svg<P: AsRef<Path>>(&self, path: P) -> Result<(), io::Error> {
        let scene = visualisation(self);
        write_svg_file(&scene, path)
    }

    /// Save log file of currently recorded tasks logs.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), io::Error> {
        let file = File::create(path)?;
        serde_json::to_writer(file, &self).expect("failed serializing");
        Ok(())
    }
}
