//! What a command keeps: what its `undo` undoes, the values it produced,
//! and its record. Wiping it runs its `undo` where its steps run, which
//! needs what it needs to be up, and then forgets the rest.

use crate::module::operations::{Keeping, Removing, UnitPlace};
use crate::module::running::{RECORD, remove_record};
use crate::module::settings::{Keeps, filled};
use crate::value::{Filled, Placeholder, Unfilled, Value};

use super::CommandSettings;
use super::produced;

impl Keeps for CommandSettings<Value> {
    fn kept(&self, _: &UnitPlace) -> Keeping {
        let mut what = Vec::new();
        if self.undo.is_some() {
            what.push("what its `undo` undoes".to_string());
        }
        if self.keeps_values && !self.produces.is_empty() {
            what.push("the values it produced".to_string());
        }
        what.push(RECORD.to_string());
        Keeping {
            what,
            runs: self.undo.is_some(),
        }
    }

    fn wipe(
        &self,
        place: UnitPlace,
        fill: &mut dyn FnMut(&Placeholder) -> Result<Filled, Unfilled>,
    ) -> Result<Removing, Unfilled> {
        // Only a command that has an `undo` uses its values to be wiped.
        let undoing = match self.undo {
            Some(_) => Some(filled(self, fill)?),
            None => None,
        };
        Ok(Box::pin(async move {
            if let Some(undoing) = undoing
                && let Some(undo) = &undoing.undo
            {
                undoing.undo(&place, undo).await?;
            }
            produced::forget(&place)?;
            remove_record(&place)
        }))
    }
}
