//! The environment's one network, `tay-<id>`: made when the first container
//! needs it, joined by every container of the environment, and removed
//! with them by `tay down`. It belongs to the environment, not to any
//! unit, so it carries the environment's label alone.

use std::collections::HashMap;

use bollard::models::NetworkCreateRequest;
use bollard::query_parameters::{InspectNetworkOptions, ListNetworksOptionsBuilder};

use super::ours::{ENVIRONMENT, Owner, is_ours};
use super::{Docker, DockerError, conflict, missing};

/// The environment's network, which carries its label.
pub struct OurNetwork {
    id: String,
    pub name: String,
    /// The host's address on it, which its containers reach the host by.
    pub gateway: Option<String>,
}

impl Docker {
    /// The network `name` of `environment`, when there is one; one of that
    /// name that is not the environment's is an error, and is left alone.
    pub async fn network(
        &self,
        environment: &str,
        name: &str,
    ) -> Result<Option<OurNetwork>, DockerError> {
        let inspected = match self
            .api
            .inspect_network(name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(inspected) => inspected,
            Err(error) if missing(&error) => return Ok(None),
            Err(error) => return Err(self.failed(format!("look at the network `{name}`"))(error)),
        };
        if !is_ours(inspected.labels.as_ref(), environment) {
            return Err(DockerError::Foreign {
                what: format!("a network named `{name}`"),
            });
        }
        let gateway = inspected
            .ipam
            .and_then(|ipam| ipam.config)
            .and_then(|config| config.into_iter().find_map(|config| config.gateway));
        let id = inspected
            .id
            .ok_or_else(|| DockerError::Other(format!("Docker gave the network `{name}` no id")))?;
        Ok(Some(OurNetwork {
            id,
            name: name.to_string(),
            gateway,
        }))
    }

    /// The network `name` of `environment`, made now if it is missing. Two
    /// units that make it at once both have it.
    pub async fn make_network(
        &self,
        environment: &str,
        name: &str,
    ) -> Result<OurNetwork, DockerError> {
        if let Some(network) = self.network(environment, name).await? {
            return Ok(network);
        }
        let owner = Owner {
            environment,
            unit: None,
        };
        let request = NetworkCreateRequest {
            name: name.to_string(),
            driver: Some("bridge".to_string()),
            labels: Some(owner.labels()),
            ..NetworkCreateRequest::default()
        };
        match self.api.create_network(request).await {
            Ok(_) => {}
            Err(error) if conflict(&error) => {}
            Err(error) => return Err(self.failed(format!("create the network `{name}`"))(error)),
        }
        self.network(environment, name).await?.ok_or_else(|| {
            DockerError::Other(format!("the network `{name}` was made and is gone at once"))
        })
    }

    /// The names of the networks that are `environment`'s.
    pub async fn networks(&self, environment: &str) -> Result<Vec<String>, DockerError> {
        let filters = HashMap::from([("label", vec![format!("{ENVIRONMENT}={environment}")])]);
        let options = ListNetworksOptionsBuilder::default()
            .filters(&filters)
            .build();
        let listed = self
            .api
            .list_networks(Some(options))
            .await
            .map_err(self.failed("list the environment's networks"))?;
        Ok(listed
            .into_iter()
            .filter(|network| is_ours(network.labels.as_ref(), environment))
            .filter_map(|network| network.name)
            .collect())
    }

    /// Removes `network`. Docker refuses while a container is on it, which
    /// tay never takes off it.
    pub async fn remove_network(&self, network: OurNetwork) -> Result<(), DockerError> {
        match self.api.remove_network(&network.id).await {
            Ok(()) => Ok(()),
            Err(error) if missing(&error) => Ok(()),
            Err(error) => Err(self
                .failed(format!("remove the network `{}`", network.name))(
                error
            )),
        }
    }
}
