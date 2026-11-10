//! Filling a unit's placeholders, from the values the units it needs give
//! and from what is known of its environment.

use std::collections::HashMap;

use crate::disk::Environment;
use crate::module::{self, Values};
use crate::value::{Filled, Placeholder, Unfilled};

/// The values each unit that has given any gives, by unit.
pub type UnitValues = HashMap<String, Values>;

/// Fills `placeholder` for a unit of `environment`, whose needs have given
/// `given`.
pub fn fill(
    placeholder: &Placeholder,
    environment: &Environment,
    given: &UnitValues,
) -> Result<Filled, Unfilled> {
    match placeholder {
        Placeholder::Port { unit, name }
        | Placeholder::Generated { unit, name, .. }
        | Placeholder::Produced { unit, name, .. } => given
            .get(unit)
            .and_then(|values| values.get(name))
            .cloned()
            .ok_or_else(|| Unfilled::Value {
                unit: unit.clone(),
                name: name.clone(),
            }),
        Placeholder::EnvironmentId => Ok(Filled::text(environment.id.as_str())),
        Placeholder::EnvironmentNetwork => Ok(Filled::text(environment.network())),
        Placeholder::HostListen => {
            module::host_listen()
                .map(Filled::text)
                .map_err(|why| Unfilled::Fact {
                    reference: "`${host.listen}`",
                    why,
                })
        }
        Placeholder::SecretSetting { value, .. } => Ok(Filled::kept(value)),
        Placeholder::Arg { name, .. } => Err(Unfilled::Argument { name: name.clone() }),
    }
}
