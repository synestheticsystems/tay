//! The settings a stack declares in `tay.yaml`.

use crate::definition::SettingDecl;
use crate::diagnostic::Diagnostics;
use crate::syntax::expr::{self, Type};
use crate::syntax::fields::{self, Fields};
use crate::syntax::names;
use crate::syntax::yaml::Node;

pub fn declarations(node: &Node, d: &mut Diagnostics) -> Vec<SettingDecl> {
    fields::keyed(node, "a setting", names::setting_name_problem, d)
        .into_iter()
        .filter_map(|(name, decl)| {
            let decl = Fields::of(
                decl,
                "a setting's declaration",
                &["default", "doc", "type", "secret"],
                d,
            )?;
            if let Some(doc) = decl.get("doc") {
                fields::text(doc, "`doc`", d);
            }
            let default = decl
                .get("default")
                .and_then(|node| expr::from_node(node, d));
            let written =
                decl.get("type")
                    .and_then(|node| match fields::text(node, "`type`", d)? {
                        "text" => Some(Type::Text),
                        "number" => Some(Type::Number),
                        "boolean" => Some(Type::Bool),
                        other => {
                            d.error(
                                node.loc,
                                format!(
                                    "`type` is `text`, `number` or `boolean`; this is `{other}`"
                                ),
                            );
                            None
                        }
                    });
            let of_default = default.as_ref().and_then(|default| default.written_type());
            if let (Some(written), Some(of_default), Some(default)) =
                (written, of_default, default.as_ref())
                && written != of_default
            {
                d.error(
                    default.loc,
                    format!(
                        "the default is {}, but `type` says the setting is {}",
                        of_default.describe(),
                        written.describe()
                    ),
                );
            }
            // With neither a default nor a `type`, as in `images.server: {}`,
            // the setting is text.
            let ty = match (&default, written.or(of_default)) {
                (_, Some(ty)) => Some(ty),
                (Some(_), None) => None,
                (None, None) => Some(Type::Text),
            };
            let secret = decl
                .get("secret")
                .and_then(|node| fields::boolean(node, "`secret`", d))
                .unwrap_or(false);
            Some(SettingDecl {
                name,
                default,
                ty,
                secret,
            })
        })
        .collect()
}
