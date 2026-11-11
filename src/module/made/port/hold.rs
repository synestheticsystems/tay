//! Holding a port unit's number: choosing it, testing it, and keeping it.
//!
//! The preferred number is tried once, the first time. After that the
//! environment keeps what it got: kept beats preferred. A kept number that
//! turns out to be taken by something other than this environment when the
//! stack comes up is chosen again, and a fixed one fails its unit. Every
//! choice is made under the lock on the machine's list of environments.

use std::fmt;
use std::io;
use std::ops::RangeInclusive;

use rand_core::{OsRng, RngCore};

use super::free::is_free;
use super::kept::{Holder, Ports};
use super::{NUMBER, PortSettings};
use crate::disk::DiskError;
use crate::module::operations::{self, Exists, Failure, Made, UnitPlace, Values, WrongType};
use crate::value::Filled;

/// The numbers a port with none of its own is given. The system hands out
/// none of them by itself, and they are below 32768, above which some
/// programs that store a port cannot.
const RANGE: RangeInclusive<u16> = 20000..=29999;

#[derive(Debug, thiserror::Error)]
enum PortError {
    #[error("it must have {number}, which is {holder}")]
    FixedTaken { number: u16, holder: Taken },
    #[error("no number from {} to {} is free", RANGE.start(), RANGE.end())]
    NoneFree,
    #[error("`{setting}` is {number}, which is not a port number")]
    NotAPort { setting: &'static str, number: i64 },
    #[error("whether {number} is free cannot be told: {source}")]
    Untested { number: u16, source: io::Error },
    #[error("it keeps no number")]
    NotKept,
    #[error("it keeps {kept}, and `fixed` asks for {fixed}")]
    NotFixed { kept: u16, fixed: u16 },
    #[error(transparent)]
    Disk(#[from] DiskError),
    #[error(transparent)]
    WrongType(#[from] WrongType),
}

/// What has a number, as well as tay can tell.
#[derive(Debug)]
enum Taken {
    By(Holder),
    /// Something on this machine is listening on it, and no environment of
    /// tay keeps it.
    Listening,
}

impl fmt::Display for Taken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Taken::By(Holder::Environment { folder }) => {
                write!(f, "kept by the environment at `{}`", folder.display())
            }
            Taken::By(Holder::Unit { name }) => write!(f, "kept by unit `{name}`"),
            Taken::Listening => f.write_str("held by a program that is not this environment's"),
        }
    }
}

impl Made for PortSettings<Filled> {
    fn make(&self, place: &UnitPlace, in_use: bool) -> Result<Exists, Failure> {
        let mut ports = Ports::lock(&place.environment).map_err(PortError::from)?;
        let unit = place.unit.as_str();
        let taken = |number| taken(&ports, number, unit, in_use);
        let (number, detail) = match (self, ports.kept(unit)) {
            (PortSettings::Fixed(fixed), _) => {
                let fixed = port_number(fixed, "fixed")?;
                if let Some(holder) = taken(fixed)? {
                    return Err(PortError::FixedTaken {
                        number: fixed,
                        holder,
                    }
                    .into());
                }
                (fixed, fixed.to_string())
            }
            (_, Some(kept)) => match taken(kept)? {
                None => (kept, format!("{kept}, kept")),
                Some(holder) => {
                    let number = choose(&ports, unit)?;
                    let detail =
                        format!("{number}, chosen again: {kept}, which it kept, is {holder}");
                    (number, detail)
                }
            },
            (PortSettings::Prefer(preferred), None) => {
                let preferred = port_number(preferred, "prefer")?;
                match taken(preferred)? {
                    None => (preferred, format!("{preferred}, as preferred")),
                    Some(holder) => {
                        let number = choose(&ports, unit)?;
                        let detail =
                            format!("{number}: {preferred}, which it prefers, is {holder}");
                        (number, detail)
                    }
                }
            }
            (PortSettings::Any, None) => {
                let number = choose(&ports, unit)?;
                (number, number.to_string())
            }
        };
        if ports.kept(unit) != Some(number) {
            ports.keep(unit, number).map_err(PortError::from)?;
        }
        Ok(Exists {
            values: values(number),
            detail,
        })
    }

    fn check(&self, place: &UnitPlace) -> Result<Exists, Failure> {
        let kept = Ports::kept_by(&place.environment).map_err(PortError::from)?;
        let kept = *kept.get(&place.unit).ok_or(PortError::NotKept)?;
        if let PortSettings::Fixed(fixed) = self {
            let fixed = port_number(fixed, "fixed")?;
            if fixed != kept {
                return Err(PortError::NotFixed { kept, fixed }.into());
            }
        }
        Ok(Exists {
            values: values(kept),
            detail: kept.to_string(),
        })
    }
}

fn values(number: u16) -> Values {
    Values::from([(NUMBER.to_string(), Filled::number(number.into()))])
}

fn port_number(value: &Filled, setting: &'static str) -> Result<u16, PortError> {
    let number = operations::number(value, setting)?;
    u16::try_from(number)
        .ok()
        .filter(|number| *number != 0)
        .ok_or(PortError::NotAPort { setting, number })
}

/// What has `number`, if anything does but `unit`. A number `unit` keeps,
/// while a unit of this environment that uses it runs, is held by that
/// unit, as well as tay can tell, and so by nothing else.
fn taken(
    ports: &Ports<'_>,
    number: u16,
    unit: &str,
    in_use: bool,
) -> Result<Option<Taken>, PortError> {
    if let Some(holder) = ports.holder(number, unit) {
        return Ok(Some(Taken::By(holder)));
    }
    if in_use && ports.kept(unit) == Some(number) {
        return Ok(None);
    }
    let free = is_free(number).map_err(|source| PortError::Untested { number, source })?;
    Ok((!free).then_some(Taken::Listening))
}

/// A number from the range that nothing has, tried from a random place in
/// it so that environments starting together do not all test the same
/// numbers first.
fn choose(ports: &Ports<'_>, unit: &str) -> Result<u16, PortError> {
    let size = u32::from(RANGE.end() - RANGE.start()) + 1;
    let start = OsRng.next_u32() % size;
    for step in 0..size {
        let offset = u16::try_from((start + step) % size).expect("an offset in the range fits");
        let number = RANGE.start() + offset;
        if taken(ports, number, unit, false)?.is_none() {
            return Ok(number);
        }
    }
    Err(PortError::NoneFree)
}
