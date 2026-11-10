//! What belongs to an environment in Docker. Every container, volume and
//! network tay creates carries the label `tay.environment`, the
//! environment's id, and every container and volume `tay.unit`, the unit's
//! name. Nothing without the environment's label is ever stopped, removed
//! or changed: this file is the one place that decides what carries it. A
//! container is started, stopped, removed or run in only through an
//! [`OurContainer`], which only [`OurContainer::of`] makes from what Docker
//! says of it, or [`OurContainer::created`] from what tay has just created
//! with the label; a volume or a network is used only once [`is_ours`]
//! says it is.

use std::collections::HashMap;

/// The label every container, volume and network tay creates carries: the
/// id of the environment it belongs to.
pub const ENVIRONMENT: &str = "tay.environment";

/// The label every container and volume tay creates carries: the unit it
/// belongs to.
pub const UNIT: &str = "tay.unit";

/// The label of a unit's container that says which of its attempts the
/// container was created for.
pub const ATTEMPT: &str = "tay.attempt";

/// The label of a container made for one step of a command, `check` or
/// `run`, that says which.
pub const STEP: &str = "tay.step";

/// The label of a unit's container that says what it was created from,
/// with every secret left out.
pub const SETTINGS: &str = "tay.settings";

/// Whom something tay creates belongs to: an environment, and one of its
/// units unless it is the environment's own, as its network is.
#[derive(Clone, Copy)]
pub struct Owner<'a> {
    pub environment: &'a str,
    pub unit: Option<&'a str>,
}

impl Owner<'_> {
    /// The labels that say so.
    pub fn labels(&self) -> HashMap<String, String> {
        let mut labels = HashMap::from([(ENVIRONMENT.to_string(), self.environment.to_string())]);
        if let Some(unit) = self.unit {
            labels.insert(UNIT.to_string(), unit.to_string());
        }
        labels
    }
}

/// A container that carries the environment's label. Only the Docker
/// layer can make one, and only from what says so.
#[derive(Clone, Debug)]
pub struct OurContainer {
    pub(super) id: String,
    name: String,
    /// The unit it belongs to.
    unit: String,
    labels: HashMap<String, String>,
}

impl OurContainer {
    /// The container, if `labels` say it is `environment`'s.
    pub(super) fn of(
        id: Option<String>,
        name: Option<&str>,
        labels: Option<HashMap<String, String>>,
        environment: &str,
    ) -> Option<OurContainer> {
        let labels = labels?;
        if labels.get(ENVIRONMENT).map(String::as_str) != Some(environment) {
            return None;
        }
        Some(OurContainer {
            id: id?,
            // Docker names a container with a `/` before its name.
            name: name?.trim_start_matches('/').to_string(),
            unit: labels.get(UNIT)?.clone(),
            labels,
        })
    }

    /// The container `create` has just made, as `owner`'s, with `labels`.
    pub(super) fn created(
        id: String,
        name: &str,
        owner: &Owner<'_>,
        labels: HashMap<String, String>,
    ) -> OurContainer {
        OurContainer {
            id,
            name: name.to_string(),
            unit: owner.unit.unwrap_or_default().to_string(),
            labels,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The unit it belongs to.
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// The label `key` it carries, if it carries one.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }
}

/// Whether something of these labels is `environment`'s: the one test of
/// whether tay may touch it.
pub(super) fn is_ours(labels: Option<&HashMap<String, String>>, environment: &str) -> bool {
    labels
        .and_then(|labels| labels.get(ENVIRONMENT))
        .map(String::as_str)
        == Some(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_carries_the_environments_label_is_ours() {
        let labels = |pairs: &[(&str, &str)]| {
            Some(
                pairs
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect::<HashMap<_, _>>(),
            )
        };
        let id = || Some("c0ffee".to_string());
        let name = Some("/tay-abc-web");
        let ours = OurContainer::of(
            id(),
            name,
            labels(&[(ENVIRONMENT, "abc"), (UNIT, "web")]),
            "abc",
        )
        .unwrap();
        assert_eq!(
            (ours.name.as_str(), ours.unit.as_str()),
            ("tay-abc-web", "web")
        );
        // Another environment's, one with no label, and one whose name
        // alone looks like tay's are not.
        assert!(
            OurContainer::of(
                id(),
                name,
                labels(&[(ENVIRONMENT, "abd"), (UNIT, "web")]),
                "abc"
            )
            .is_none()
        );
        assert!(OurContainer::of(id(), name, labels(&[(UNIT, "web")]), "abc").is_none());
        assert!(OurContainer::of(id(), name, None, "abc").is_none());
        assert!(!is_ours(
            labels(&[("tay.environmentx", "abc")]).as_ref(),
            "abc"
        ));
    }
}
