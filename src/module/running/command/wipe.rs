//! What a command keeps: what its `undo` undoes, the values it produced,
//! and its record. Wiping it runs its `undo` where its steps run, which
//! needs what it needs to be up, and then forgets the rest, what was kept
//! for an `undo` to run once it leaves the plan among it.

use crate::module::operations::{Keeping, PartlyRemoved, Removing, UnitPlace};
use crate::module::running::{RECORD, remove_record};
use crate::module::settings::{Keeps, filled};
use crate::value::{Filled, Placeholder, Unfilled, Value};

use super::{CommandSettings, kept_undo, produced};

impl Keeps for CommandSettings<Value> {
    fn kept(&self, _: &UnitPlace) -> Keeping {
        let mut what = Vec::new();
        if self.undo.is_some() {
            what.push("what its `undo` undoes".to_string());
        }
        if self.keeps && !self.produces.is_empty() {
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
        let values_kept = self.keeps && !self.produces.is_empty();
        Ok(Box::pin(async move {
            let mut removed = 0;
            if let Some(undoing) = undoing
                && let Some(undo) = &undoing.undo
            {
                undoing
                    .steps
                    .undo(&place, undo)
                    .await
                    .map_err(PartlyRemoved::after(removed))?;
                kept_undo::forget(&place).map_err(PartlyRemoved::after(removed))?;
                removed += 1;
            }
            produced::forget(&place).map_err(PartlyRemoved::after(removed))?;
            removed += usize::from(values_kept);
            remove_record(&place).map_err(PartlyRemoved::after(removed))
        }))
    }
}
