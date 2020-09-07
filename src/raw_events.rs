//! We define here all raw events.
//! Events which are very fast to log and logged on a per thread basis.
//! These events will be post-processed after execution in order to generate
//! a tasks graph.
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::collections::HashMap;

/// unique thread identifier
pub(crate) type ThreadId = usize;
/// unique subgraph identifier
pub(crate) type SubGraphId = usize;
/// unique task identifier
pub(crate) type TaskId = usize;
/// unique iterator identifier (currently unused, will come back later)
pub(crate) type IteratorId = usize;
/// at which time (in nanoseconds) does the event happen
pub(crate) type TimeStamp = u64;

use lazy_static::lazy_static;
lazy_static! {
    static ref START_TIME: std::time::Instant = std::time::Instant::now();
}

/// Return number of nano seconds since start.
pub(crate) fn now() -> TimeStamp {
    START_TIME.elapsed().as_nanos() as TimeStamp
}

/// All types of raw events we can log.
#[derive(Debug)]
pub(crate) enum RayonEvent {
    /// A task starts.
    TaskStart(TaskId, TimeStamp),
    /// Active task ends.
    TaskEnd(TimeStamp),
    /// Direct link in the graph between two tasks (active one and given one).
    Child(TaskId),
    /// Start a subgraph.
    SubgraphStart(&'static str),
    /// End a subgraph and register a work amount.
    SubgraphEnd(&'static str, usize), // TODO: do we need the label here ?
}

#[derive(Debug, Clone)]
pub(crate) enum RawEvent {
    /// A task starts.
    TaskStart(TaskId, TimeStamp),
    /// Active task ends.
    TaskEnd(TimeStamp),
    /// Direct link in the graph between two tasks (active one and given one).
    Child(TaskId),
    /// Start a subgraph.
    SubgraphStart(usize),
    /// End a subgraph and register a work amount.
    SubgraphEnd(usize, usize),
}

impl RawEvent {
    pub(crate) fn new(rayon_event: &RayonEvent, strings: &HashMap<&str, usize>) -> RawEvent {
        match rayon_event {
            RayonEvent::TaskStart(id, time) => RawEvent::TaskStart(*id, *time),
            RayonEvent::TaskEnd(time) => RawEvent::TaskEnd(*time),
            RayonEvent::Child(id) => RawEvent::Child(*id),
            RayonEvent::SubgraphStart(label) => RawEvent::SubgraphStart(strings[label]),
            RayonEvent::SubgraphEnd(label, size) => RawEvent::SubgraphEnd(strings[label], *size),
        }
    }
    pub(crate) fn write_to<W: std::io::Write>(&self, destination: &mut W) -> std::io::Result<()> {
        match self {
            RawEvent::TaskStart(id, time) => {
                destination.write(&[2u8])?;
                destination.write_u64::<LittleEndian>(*id as u64)?;
                destination.write_u64::<LittleEndian>(*time)?;
            }
            RawEvent::TaskEnd(time) => {
                destination.write(&[3u8])?;
                destination.write_u64::<LittleEndian>(*time)?;
            }
            RawEvent::Child(id) => {
                destination.write(&[4u8])?;
                destination.write_u64::<LittleEndian>(*id as u64)?;
            }
            RawEvent::SubgraphStart(label) => {
                destination.write(&[5u8])?;
                destination.write_u64::<LittleEndian>(*label as u64)?;
            }
            RawEvent::SubgraphEnd(label, size) => {
                destination.write(&[6u8])?;
                destination.write_u64::<LittleEndian>(*label as u64)?;
                destination.write_u64::<LittleEndian>(*size as u64)?;
            }
        }
        Ok(())
    }
    pub(crate) fn read_from<R: std::io::Read>(source: &mut R) -> std::io::Result<Self> {
        let mut byte = [0u8];
        source.read_exact(&mut byte)?;
        let event = match byte[0] {
            2 => RawEvent::TaskStart(
                source.read_u64::<LittleEndian>()? as TaskId,
                source.read_u64::<LittleEndian>()?,
            ),
            3 => RawEvent::TaskEnd(source.read_u64::<LittleEndian>()?),
            4 => RawEvent::Child(source.read_u64::<LittleEndian>()? as TaskId),
            5 => RawEvent::SubgraphStart(source.read_u64::<LittleEndian>()? as SubGraphId),
            6 => RawEvent::SubgraphEnd(
                source.read_u64::<LittleEndian>()? as SubGraphId,
                source.read_u64::<LittleEndian>()? as usize,
            ),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid event discriminant",
                ))
            }
        };
        Ok(event)
    }
    pub(crate) fn time(&self) -> TimeStamp {
        match *self {
            RawEvent::TaskStart(_, t) => t,
            RawEvent::TaskEnd(t) => t,
            _ => 0,
        }
    }
}

impl RayonEvent {
    /// return event time or 0 if none
    pub(crate) fn time(&self) -> TimeStamp {
        match *self {
            RayonEvent::TaskStart(_, t) => t,
            RayonEvent::TaskEnd(t) => t,
            _ => 0,
        }
    }
}
