//! The settings a stack declares in `tay.yaml`.

use crate::definition::{SettingDecl, Written};
use crate::diagnostic::Diagnostics;
use crate::syntax::expr::Type;
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
            let default = decl.get("default").and_then(|node| {
                Some(Written {
                    value: fields::literal(node, "a setting's value", d)?,
                    loc: node.loc,
                })
            });
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
            if let (Some(written), Some(default)) = (written, &default)
                && written != default.value.ty()
            {
                d.error(
                    default.loc,
                    format!(
                        "the default is {}, but `type` says the setting is {}",
                        default.value.ty().describe(),
                        written.describe()
                    ),
                );
            }
            // With neither a default nor a `type`, as in `images.server: {}`,
            // the setting is text.
            let ty = written
                .or(default.as_ref().map(|default| default.value.ty()))
                .unwrap_or(Type::Text);
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
