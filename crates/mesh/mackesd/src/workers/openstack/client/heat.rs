//! IAC-4 — the native **Heat** (`OpenStack` orchestration) control loop over
//! IAC-1's standard resource seam.
//!
//! Design #5/#6/#21: Heat *is* the `IaC` engine. This module drives the standard
//! Heat REST API — `GET /stacks/{id}` + its `resources`/`events`/`template`
//! sub-resources, `PUT …/preview` (dry-run diff), `POST …/actions {check}`
//! (drift), `POST /stacks` / `PUT` / `DELETE` (create/update/delete), and a
//! reverse-generate that lists live resources and emits a HOT template. It reuses
//! [`OpenStackClient`]'s existing [`ResourceApi`] + [`Session`] seam (no parallel
//! client), so the same auth + endpoint-resolution the Resources tab uses carries
//! the Heat loop.
//!
//! Honest, never fake (§7): the read loop (`heat_show`) makes real Heat calls and
//! degrades to `Unconfigured`/transport-failure; the sub-resource GETs
//! (resources/events/template) are best-effort — a section Heat doesn't return is
//! honestly empty, never fabricated. The mutating + preview + reverse ops all
//! reach the real Heat API through the same seam.

use mackes_mesh_types::openstack::{
    reverse_generate_hot, HeatPreview, HeatStackDetail, ResourceTable,
};

use super::keystone::Session;
use super::resource::{ResourceApi, ResourceRef, ResourceRequest, Verb};
use super::{ClientError, HeatSource, OpenStackClient};

/// The Heat catalog service type (`orchestration`) — the endpoint the stack calls
/// resolve against.
const HEAT_SERVICE: &str = "orchestration";

/// Render an editable template buffer to the `template` field a Heat body wants:
/// the parsed object when the buffer is valid JSON HOT, else the raw string
/// (Heat parses a string template as YAML). Either shape is accepted by the API,
/// so a YAML-authored buffer rides as a string and a JSON one as an object.
fn template_field(template: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(template.trim())
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::Value::String(template.to_string()))
}

impl OpenStackClient {
    /// Issue a Heat `GET` at `collection` (a full path under the orchestration
    /// endpoint, e.g. `stacks/<id>` or `stacks/<name>/<id>/resources`), returning
    /// the body on a 2xx or a typed transport error otherwise.
    fn heat_get(&self, session: &Session, collection: &str) -> Result<String, ClientError> {
        let req = ResourceRequest {
            verb: Verb::List,
            target: ResourceRef {
                service_type: HEAT_SERVICE.to_string(),
                collection: collection.to_string(),
                id: None,
            },
            body: None,
            query: Vec::new(),
        };
        let resp = self.resource_api().call(session, &req)?;
        if !resp.is_success() {
            return Err(ClientError::Transport(format!(
                "HTTP {} on GET {collection}",
                resp.status
            )));
        }
        Ok(resp.body)
    }

    /// A best-effort Heat sub-resource GET — `Some(body)` on a 2xx, `None`
    /// otherwise (a missing sub-resource leaves that stack-detail section honestly
    /// empty rather than failing the whole show).
    fn heat_get_opt(&self, session: &Session, collection: &str) -> Option<String> {
        self.heat_get(session, collection).ok()
    }

    /// Issue a Heat mutating/preview request (`verb` at `collection`[/`id`] with
    /// `body`), returning the response on a 2xx or a typed transport error with
    /// the response body (Heat's error message) otherwise.
    fn heat_send(
        &self,
        session: &Session,
        verb: Verb,
        collection: &str,
        id: Option<&str>,
        body: serde_json::Value,
    ) -> Result<super::resource::ResourceResponse, ClientError> {
        let req = ResourceRequest {
            verb,
            target: ResourceRef {
                service_type: HEAT_SERVICE.to_string(),
                collection: collection.to_string(),
                id: id.map(str::to_string),
            },
            body: Some(body),
            query: Vec::new(),
        };
        let resp = self.resource_api().call(session, &req)?;
        if !resp.is_success() {
            return Err(ClientError::Transport(format!(
                "HTTP {} on {} {collection} \u{2014} {}",
                resp.status,
                verb.http_method(),
                resp.body.trim()
            )));
        }
        Ok(resp)
    }
}

impl HeatSource for OpenStackClient {
    fn heat_show(&self, stack: &str) -> Result<HeatStackDetail, ClientError> {
        let session = self.authenticate()?;
        // The stack detail (id-only path 302-redirects to the canonical name/id
        // URL, which reqwest follows) — the mandatory read.
        let detail_body = self.heat_get(&session, &format!("stacks/{stack}"))?;
        let mut detail = HeatStackDetail::from_stack_json(&detail_body)
            .map_err(|e| ClientError::Catalog(e.to_string()))?;
        // The sub-resources ride the canonical name/id path (no redirect); each is
        // best-effort so one missing section never hides the rest.
        let canon = format!("stacks/{}/{}", detail.stack_name, detail.stack_id);
        if let Some(body) = self.heat_get_opt(&session, &format!("{canon}/resources")) {
            detail = detail.with_resources_json(&body);
        }
        if let Some(body) = self.heat_get_opt(&session, &format!("{canon}/events")) {
            detail = detail.with_events_json(&body);
        }
        if let Some(body) = self.heat_get_opt(&session, &format!("{canon}/template")) {
            detail = detail.with_template_json(&body);
        }
        Ok(detail)
    }

    fn heat_preview(
        &self,
        stack_name: &str,
        stack_id: &str,
        template: &str,
    ) -> Result<HeatPreview, ClientError> {
        let session = self.authenticate()?;
        // PUT /stacks/{name}/{id}/preview — the id segment carries `preview`.
        let body = serde_json::json!({
            "stack_name": stack_name,
            "template": template_field(template),
        });
        let resp = self.heat_send(
            &session,
            Verb::Update,
            &format!("stacks/{stack_name}/{stack_id}"),
            Some("preview"),
            body,
        )?;
        HeatPreview::from_json(&resp.body).map_err(|e| ClientError::Catalog(e.to_string()))
    }

    fn heat_check(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        let session = self.authenticate()?;
        // POST /stacks/{name}/{id}/actions {"check": null}
        self.heat_send(
            &session,
            Verb::Create,
            &format!("stacks/{stack_name}/{stack_id}/actions"),
            None,
            serde_json::json!({ "check": serde_json::Value::Null }),
        )?;
        Ok(())
    }

    fn heat_create(&self, stack_name: &str, template: &str) -> Result<String, ClientError> {
        let session = self.authenticate()?;
        // POST /stacks {"stack_name": …, "template": …}
        let body = serde_json::json!({
            "stack_name": stack_name,
            "template": template_field(template),
        });
        let resp = self.heat_send(&session, Verb::Create, "stacks", None, body)?;
        // The create response carries the new stack's id under `stack.id`.
        let id = serde_json::from_str::<serde_json::Value>(resp.body.trim())
            .ok()
            .and_then(|v| {
                v.get("stack")
                    .and_then(|s| s.get("id"))
                    .and_then(|i| i.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        Ok(id)
    }

    fn heat_update(
        &self,
        stack_name: &str,
        stack_id: &str,
        template: &str,
    ) -> Result<(), ClientError> {
        let session = self.authenticate()?;
        // PUT /stacks/{name}/{id} {"template": …}
        let body = serde_json::json!({ "template": template_field(template) });
        self.heat_send(
            &session,
            Verb::Update,
            &format!("stacks/{stack_name}"),
            Some(stack_id),
            body,
        )?;
        Ok(())
    }

    fn heat_delete(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        let session = self.authenticate()?;
        // DELETE /stacks/{name}/{id}
        self.heat_send(
            &session,
            Verb::Delete,
            &format!("stacks/{stack_name}"),
            Some(stack_id),
            serde_json::Value::Null,
        )?;
        Ok(())
    }

    fn heat_reverse(&self, services: &[(String, String)]) -> Result<String, ClientError> {
        // One auth, then list each service's resources over the shared seam and
        // emit a HOT capturing them. A service that errors is skipped (best-effort
        // capture — a partial reality is still honest), but a total auth failure
        // propagates.
        let session = self.authenticate()?;
        let mut tables: Vec<ResourceTable> = Vec::new();
        for (service_type, collection) in services {
            let req = ResourceRequest {
                verb: Verb::List,
                target: ResourceRef {
                    service_type: service_type.clone(),
                    collection: collection.clone(),
                    id: None,
                },
                body: None,
                query: Vec::new(),
            };
            let Ok(resp) = self.resource_api().call(&session, &req) else {
                continue;
            };
            if !resp.is_success() {
                continue;
            }
            if let Ok(table) =
                ResourceTable::from_collection_json(service_type, collection, &resp.body)
            {
                tables.push(table);
            }
        }
        Ok(reverse_generate_hot(&tables))
    }
}

impl HeatSource for super::LiveOpenStack {
    fn heat_show(&self, stack: &str) -> Result<HeatStackDetail, ClientError> {
        OpenStackClient::live(super::config::load_default()?).heat_show(stack)
    }

    fn heat_preview(
        &self,
        stack_name: &str,
        stack_id: &str,
        template: &str,
    ) -> Result<HeatPreview, ClientError> {
        OpenStackClient::live(super::config::load_default()?)
            .heat_preview(stack_name, stack_id, template)
    }

    fn heat_check(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        OpenStackClient::live(super::config::load_default()?).heat_check(stack_name, stack_id)
    }

    fn heat_create(&self, stack_name: &str, template: &str) -> Result<String, ClientError> {
        OpenStackClient::live(super::config::load_default()?).heat_create(stack_name, template)
    }

    fn heat_update(
        &self,
        stack_name: &str,
        stack_id: &str,
        template: &str,
    ) -> Result<(), ClientError> {
        OpenStackClient::live(super::config::load_default()?)
            .heat_update(stack_name, stack_id, template)
    }

    fn heat_delete(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        OpenStackClient::live(super::config::load_default()?).heat_delete(stack_name, stack_id)
    }

    fn heat_reverse(&self, services: &[(String, String)]) -> Result<String, ClientError> {
        OpenStackClient::live(super::config::load_default()?).heat_reverse(services)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::client::testkit::{FakeKeystone, FakeProbe};
    use mackes_mesh_types::openstack::ServiceCatalog;

    /// A composed client whose Keystone answers a session cataloging Heat, and
    /// whose resource calls are… the real `TokenRestApi`. These tests exercise the
    /// pure `template_field` shaping + the reverse-generate wiring that doesn't
    /// need a live endpoint; the live Heat round-trip is the IAC-6 smoke.
    fn client() -> OpenStackClient {
        let cfg = super::super::config::CloudConfig {
            cloud: "mesh".into(),
            auth_url: "http://keystone.mesh:5000/v3".into(),
            username: "operator".into(),
            password: "pw".into(),
            project_name: Some("mesh".into()),
            project_domain: "Default".into(),
            user_domain: "Default".into(),
            region_name: Some("RegionOne".into()),
            interface: mackes_mesh_types::openstack::EndpointInterface::Public,
        };
        let session = Session {
            token: "tok".into(),
            catalog: ServiceCatalog::from_keystone_token_json(
                r#"{"token":{"catalog":[
                    {"type":"orchestration","name":"heat","endpoints":[
                        {"interface":"public","url":"http://heat.mesh:8004/v1/p","region":"RegionOne"}
                    ]}
                ]}}"#,
            )
            .unwrap(),
            expires_at: None,
        };
        OpenStackClient::new(
            cfg,
            Box::new(FakeKeystone::ok(session)),
            Box::new(FakeProbe::new()),
        )
    }

    #[test]
    fn template_field_uses_an_object_for_json_and_a_string_for_yaml() {
        // A JSON HOT buffer rides as an object.
        let obj = template_field(r#"{"heat_template_version":"2021-04-16"}"#);
        assert!(obj.is_object());
        // A YAML buffer (the common case) rides as a string Heat parses.
        let yaml = template_field("heat_template_version: 2021-04-16\nresources: {}\n");
        assert!(yaml.is_string());
    }

    #[test]
    fn heat_reverse_with_no_services_emits_an_empty_but_valid_hot() {
        // No services → a valid HOT skeleton (honest empty), never an error.
        let hot = client().heat_reverse(&[]).expect("reverse");
        assert!(hot.starts_with("heat_template_version:"));
        assert!(hot.contains("no reverse-generable resources"));
    }
}
