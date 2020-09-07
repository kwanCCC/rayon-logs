//! `LoggedPool` structure for logging raw tasks events.
#![macro_use]

// we can now use performance counters to tag subgraphs
#[cfg(feature = "perf")]
use perfcnt::linux::PerfCounterBuilderLinux;
#[cfg(feature = "perf")]
use perfcnt::linux::{CacheId, CacheOpId, CacheOpResultId, HardwareEventType, SoftwareEventType};
#[cfg(feature = "perf")]
use perfcnt::{AbstractPerfCounter, PerfCounter};

use crate::raw_events::{now, RawEvent, TaskId};
use rayon;
use rayon::FnContext;
use std::sync::atomic::{AtomicUsize, Ordering};

/// We use an atomic usize to generate unique ids for tasks.
/// We start at 1 since initial task (0) is created manually.
pub(crate) static NEXT_TASK_ID: AtomicUsize = AtomicUsize::new(1);
/// We use an atomic usize to generate unique ids for iterators.
pub(crate) static NEXT_ITERATOR_ID: AtomicUsize = AtomicUsize::new(0);

/// get an id for a new task and increment global tasks counter.
pub fn next_task_id() -> TaskId {
    NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst)
}

/// get an id for a new iterator and increment global iterators counter.
pub fn next_iterator_id() -> usize {
    NEXT_ITERATOR_ID.fetch_add(1, Ordering::SeqCst)
}

/// Add given event to logs of current thread.
pub(crate) fn log(event: RawEvent<&'static str>) {
    crate::raw_logs::THREAD_LOGS.with(|l| l.push(event))
}

/// Logs several events at once (with decreased cost).
macro_rules! logs {
    ($($x:expr ), +) => {
        $crate::raw_logs::THREAD_LOGS.with(|l| {
            $(
                l.push($x);
              )*
        })
    }
}

/// We tag all the tasks that op makes as one subgraph.
///
/// `work_type` is a str tag and `work_amount` an integer specifying the expected algorithmic cost
/// (should not be zero).
/// As we know the work and execution time we can compute an execution speed for each subgraph.
/// When different graphs are tagged with the same tag we can then compare their speeds.
/// Slow graphs will see their displayed colors darkened.
/// You can also hover on tasks to display their speeds.
///
/// Example:
///
/// ```
/// use rayon_logs::{join, subgraph};
///
/// fn manual_max(slice: &[u32]) -> u32 {
///     if slice.len() < 200_000 {
///         subgraph("max", slice.len(), || slice.iter().max().cloned().unwrap())
///     } else {
///         let middle = slice.len() / 2;
///         let (left, right) = slice.split_at(middle);
///         let (mleft, mright) = join(|| manual_max(left), || manual_max(right));
///         std::cmp::max(mleft, mright)
///     }
/// }
///
/// let v: Vec<u32> = (0..2_000_000).collect();
/// let max = manual_max(&v);
/// assert_eq!(max, v.last().cloned().unwrap());
/// ```
///
/// <div>
/// <img
/// src="http://www-id.imag.fr/Laboratoire/Membres/Wagner_Frederic/images/downgraded_manual_max.svg"/>
/// </div>
///
/// Using it we obtain the graph below.
/// On the real file you can hover but javascript and toggle the display of the different tags but
/// it is disabled with rustdoc so I downgraded the file
/// for this display.
pub fn subgraph<OP, R>(work_type: &'static str, work_amount: usize, op: OP) -> R
where
    OP: FnOnce() -> R,
{
    custom_subgraph(work_type, || (), |_| work_amount, op)
}

/// Same as the subgraph function, but we can log a hardware event
///
/// (from: https://github.com/gz/rust-perfcnt)
///
/// Events:
///
/// * ```HardwareEventType::CPUCycles```
///
/// * ```HardwareEventType::Instructions```
///
/// * ```HardwareEventType::CacheReferences```
///
/// * ```HardwareEventType::CacheMisses```
///
/// * ```HardwareEventType::BranchInstructions```
///
/// * ```HardwareEventType::BranchMisses```
///
/// * ```HardwareEventType::BusCycles```
///
/// * ```HardwareEventType::StalledCyclesFrontend```
///
/// * ```HardwareEventType::StalledCyclesBackend```
///
/// * ```HardwareEventType::RefCPUCycles```
///
/// You will have to import the events from rayon_logs
/// and to use the nightly version of the compiler.
/// note that It is **freaking slow**: 1 full second to set up the counter.
#[cfg(feature = "perf")]
pub fn subgraph_hardware_event<OP, R>(tag: &'static str, event: HardwareEventType, op: OP) -> R
where
    OP: FnOnce() -> R,
{
    custom_subgraph(
        tag,
        || {
            let pc: PerfCounter = PerfCounterBuilderLinux::from_hardware_event(event)
                .exclude_idle()
                .exclude_kernel()
                .finish()
                .expect("Could not create counter");
            pc.start().expect("Can not start the counter");
            pc
        },
        |mut pc| {
            pc.stop().expect("Can not stop the counter");
            let counted_value = pc.read().unwrap() as usize;
            pc.reset().expect("Can not reset the counter");
            counted_value
        },
        op,
    )
}

/// Same as the subgraph function, but we can log a software event
///
/// (from: https://github.com/gz/rust-perfcnt)
///
/// Events:
///
/// * ```SoftwareEventType::CpuClock```
///
/// * ```SoftwareEventType::TaskClock```
///
/// * ```SoftwareEventType::PageFaults```
///
/// * ```SoftwareEventType::CacheMisses```
///
/// * ```SoftwareEventType::ContextSwitches```
///
/// * ```SoftwareEventType::CpuMigrations```
///
/// * ```SoftwareEventType::PageFaultsMin```
///
/// * ```SoftwareEventType::PageFaultsMin```
///
/// * ```SoftwareEventType::PageFaultsMaj```
///
/// * ```SoftwareEventType::AlignmentFaults```
///
/// * ```SoftwareEventType::EmulationFaults```
///
/// You will have to import the events from rayon_logs
/// and to use the nightly version of the compiler
#[cfg(feature = "perf")]
pub fn subgraph_software_event<OP, R>(tag: &'static str, event: SoftwareEventType, op: OP) -> R
where
    OP: FnOnce() -> R,
{
    //TODO: avoid code duplication by abstracting over events
    custom_subgraph(
        tag,
        || {
            let pc: PerfCounter = PerfCounterBuilderLinux::from_software_event(event)
                .exclude_idle()
                .exclude_kernel()
                .finish()
                .expect("Could not create counter");
            pc.start().expect("Can not start the counter");
            pc
        },
        |mut pc| {
            pc.stop().expect("Can not stop the counter");
            let counted_value = pc.read().unwrap() as usize;
            pc.reset().expect("Can not reset the counter");
            counted_value
        },
        op,
    )
}

/// Same as the subgraph function, but we can log a cache event
///
/// (from: https://github.com/gz/rust-perfcnt)
///
/// CacheId:
///
/// * ```CacheId::L1D```
///
/// * ```CacheId::L1I```
///
/// * ```CacheId::LL```
///
/// * ```CacheId::DTLB```
///
/// * ```CacheId::ITLB```
///
/// * ```CacheId::BPU```
///
/// * ```CacheId::Node```
///
/// CacheOpId:
///
/// * ```CacheOpId::Read```
///
/// * ```CacheOpId::Write```
///
/// * ```CacheOpId::Prefetch```
///
/// CacheOpResultId:
///
/// * ```CacheOpResultId::Access```
///
/// * ```CacheOpResultId::Miss```
///
///
/// You will have to import the events from rayon_logs
/// and to use the nightly version of the compiler
///
#[cfg(feature = "perf")]
pub fn subgraph_cache_event<OP, R>(
    tag: &'static str,
    cache_id: CacheId,
    cache_op_id: CacheOpId,
    cache_op_result_id: CacheOpResultId,
    op: OP,
) -> R
where
    OP: FnOnce() -> R,
{
    //TODO: avoid code duplication by abstracting over events
    custom_subgraph(
        tag,
        || {
            let pc: PerfCounter = PerfCounterBuilderLinux::from_cache_event(
                cache_id,
                cache_op_id,
                cache_op_result_id,
            )
            .exclude_idle()
            .exclude_kernel()
            .finish()
            .expect("Could not create counter");
            pc.start().expect("Can not start the counter");
            pc
        },
        |mut pc| {
            pc.stop().expect("Can not stop the counter");
            let counted_value = pc.read().unwrap() as usize;
            pc.reset().expect("Can not reset the counter");
            counted_value
        },
        op,
    )
}

/// Tag a subgraph with a custom value.
/// The start function will be called just before running the graph and produce an S.
/// The end function will be called just after running the graph on this S and produce a usize
/// which will the be stored for display.
pub fn custom_subgraph<OP, R, START, END, S>(tag: &'static str, start: START, end: END, op: OP) -> R
where
    OP: FnOnce() -> R,
    START: FnOnce() -> S,
    END: FnOnce(S) -> usize,
{
    let s = start();
    start_subgraph(tag);
    let r = op();
    let measured_value = end(s);
    end_subgraph(tag, measured_value);
    r
}

/// Stop current task (virtually) and start a subgraph.
/// You most likely don't need to call this function directly but `subgraph` instead.
pub fn start_subgraph(tag: &'static str) {
    let subgraph_start_task_id = next_task_id();
    logs!(
        // log child's work and dependencies.
        RawEvent::Child(subgraph_start_task_id),
        // end current task
        RawEvent::TaskEnd(now()),
        // execute full sequential task
        RawEvent::TaskStart(subgraph_start_task_id, now()),
        RawEvent::SubgraphStart(tag)
    );
}

/// Stop current task (virtually) and end a subgraph.
/// You most likely don't need to call this function directly but `subgraph` instead.
pub fn end_subgraph(tag: &'static str, measured_value: usize) {
    let continuation_task_id = next_task_id();
    logs!(
        RawEvent::SubgraphEnd(tag, measured_value),
        RawEvent::Child(continuation_task_id),
        RawEvent::TaskEnd(now()),
        // start continuation task
        RawEvent::TaskStart(continuation_task_id, now())
    );
}

/// Identical to `join`, except that the closures have a parameter
/// that provides context for the way the closure has been called,
/// especially indicating whether they're executing on a different
/// thread than where `join_context` was called.  This will occur if
/// the second job is stolen by a different thread, or if
/// `join_context` was called from outside the thread pool to begin
/// with.
pub fn join_context<A, B, RA, RB>(oper_a: A, oper_b: B) -> (RA, RB)
where
    A: FnOnce(FnContext) -> RA + Send,
    B: FnOnce(FnContext) -> RB + Send,
    RA: Send,
    RB: Send,
{
    let id_c = next_task_id();
    let id_a = next_task_id();
    let ca = |c| {
        log(RawEvent::TaskStart(id_a, now()));
        let result = oper_a(c);
        logs!(RawEvent::Child(id_c), RawEvent::TaskEnd(now()));
        result
    };

    let id_b = next_task_id();
    let cb = |c| {
        log(RawEvent::TaskStart(id_b, now()));
        let result = oper_b(c);
        logs!(RawEvent::Child(id_c), RawEvent::TaskEnd(now()));
        result
    };

    logs!(
        RawEvent::Child(id_a),
        RawEvent::Child(id_b),
        RawEvent::TaskEnd(now())
    );
    let r = rayon::join_context(ca, cb);
    log(RawEvent::TaskStart(id_c, now()));
    r
}

/// Takes two closures and *potentially* runs them in parallel. It
/// returns a pair of the results from those closures.
///
/// Conceptually, calling `join()` is similar to spawning two threads,
/// one executing each of the two closures. However, the
/// implementation is quite different and incurs very low
/// overhead. The underlying technique is called "work stealing": the
/// Rayon runtime uses a fixed pool of worker threads and attempts to
/// only execute code in parallel when there are idle CPUs to handle
/// it.
///
/// When `join` is called from outside the thread pool, the calling
/// thread will block while the closures execute in the pool.  When
/// `join` is called within the pool, the calling thread still actively
/// participates in the thread pool. It will begin by executing closure
/// A (on the current thread). While it is doing that, it will advertise
/// closure B as being available for other threads to execute. Once closure A
/// has completed, the current thread will try to execute closure B;
/// if however closure B has been stolen, then it will look for other work
/// while waiting for the thief to fully execute closure B. (This is the
/// typical work-stealing strategy).
///
/// # Examples
///
/// This example uses join to perform a quick-sort (note this is not a
/// particularly optimized implementation: if you **actually** want to
/// sort for real, you should prefer [the `par_sort` method] offered
/// by Rayon).
///
/// [the `par_sort` method]: ../rayon/slice/trait.ParallelSliceMut.html#method.par_sort
///
/// ```rust
/// let mut v = vec![5, 1, 8, 22, 0, 44];
/// quick_sort(&mut v);
/// assert_eq!(v, vec![0, 1, 5, 8, 22, 44]);
///
/// fn quick_sort<T:PartialOrd+Send>(v: &mut [T]) {
///    if v.len() > 1 {
///        let mid = partition(v);
///        let (lo, hi) = v.split_at_mut(mid);
///        rayon::join(|| quick_sort(lo),
///                    || quick_sort(hi));
///    }
/// }
///
/// // Partition rearranges all items `<=` to the pivot
/// // item (arbitrary selected to be the last item in the slice)
/// // to the first half of the slice. It then returns the
/// // "dividing point" where the pivot is placed.
/// fn partition<T:PartialOrd+Send>(v: &mut [T]) -> usize {
///     let pivot = v.len() - 1;
///     let mut i = 0;
///     for j in 0..pivot {
///         if v[j] <= v[pivot] {
///             v.swap(i, j);
///             i += 1;
///         }
///     }
///     v.swap(i, pivot);
///     i
/// }
/// ```
///
/// # Warning about blocking I/O
///
/// The assumption is that the closures given to `join()` are
/// CPU-bound tasks that do not perform I/O or other blocking
/// operations. If you do perform I/O, and that I/O should block
/// (e.g., waiting for a network request), the overall performance may
/// be poor.  Moreover, if you cause one closure to be blocked waiting
/// on another (for example, using a channel), that could lead to a
/// deadlock.
///
/// # Panics
///
/// No matter what happens, both closures will always be executed.  If
/// a single closure panics, whether it be the first or second
/// closure, that panic will be propagated and hence `join()` will
/// panic with the same panic value. If both closures panic, `join()`
/// will panic with the panic value from the first closure.
pub fn join<A, B, RA, RB>(oper_a: A, oper_b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    let id_c = next_task_id();
    let id_a = next_task_id();
    let ca = || {
        log(RawEvent::TaskStart(id_a, now()));
        let result = oper_a();
        logs!(RawEvent::Child(id_c), RawEvent::TaskEnd(now()));
        result
    };

    let id_b = next_task_id();
    let cb = || {
        log(RawEvent::TaskStart(id_b, now()));
        let result = oper_b();
        logs!(RawEvent::Child(id_c), RawEvent::TaskEnd(now()));
        result
    };

    logs!(
        RawEvent::Child(id_a),
        RawEvent::Child(id_b),
        RawEvent::TaskEnd(now())
    );
    let r = rayon::join(ca, cb);
    log(RawEvent::TaskStart(id_c, now()));
    r
}
