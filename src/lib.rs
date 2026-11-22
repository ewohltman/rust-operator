use gateway_api::httproutes::{
    HttpRoute, HttpRouteParentRefs, HttpRouteRules, HttpRouteRulesBackendRefs,
    HttpRouteRulesMatches, HttpRouteRulesMatchesPath, HttpRouteRulesMatchesPathType, HttpRouteSpec,
};
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStatus};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, Namespace, NamespaceSpec, PodSpec,
    PodTemplateSpec, Service, ServicePort, ServiceSpec, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    Condition, LabelSelector, ObjectMeta, OwnerReference, Time,
};
use kube::runtime::events::Recorder;
use kube::{Client, CustomResource, Resource};
use kube::{ResourceExt, config};
use schemars::{JsonSchema, json_schema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing_subscriber::EnvFilter;

/// A static web page served by nginx.
#[derive(
    CustomResource, Serialize, Deserialize, Default, Debug, PartialEq, Eq, Clone, JsonSchema,
)]
#[kube(
    group = "demo.ephemeral-roles.net",
    version = "v1alpha1",
    kind = "Webpage",
    namespaced,
    doc = "An HTML webpage.",
    status = "WebpageStatus",
    shortname = "wp",
    derive = "PartialEq",
    derive = "Default",
    printcolumn(
        name = "Ready",
        type_ = "string",
        json_path = r#".status.conditions[?(@.type=="Ready")].status"#,
        description = "Whether the resource is ready",
    )
)]
#[serde(rename_all = "camelCase")]
pub struct WebpageSpec {
    /// The HTML document to serve as `index.html`.
    #[schemars(length(max = MAX_HTML_BYTES))]
    #[kube(validation = Rule::new("self.lowerAscii().startsWith('<!doctype html>')").message("not a full HTML document")
    )]
    pub html: String,
    /// How many nginx pods to run at the same time.
    #[serde(default = "default_replicas")]
    #[schemars(range(min = 0, max = 10))]
    pub replicas: i32,
}

fn default_replicas() -> i32 {
    1
}

/// Observed state of a `Webpage`, written only by the operator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebpageStatus {
    /// Ready pods, as reported by the Deployment.
    pub ready_replicas: i32,
    /// The `metadata.generation` this status was computed from.
    pub observed_generation: Option<i64>,
    /// Represents the latest available observations of a webpage's current state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "conditions")]
    pub conditions: Vec<Condition>,
}

fn conditions(_: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
    json_schema!({
        "type": "array",
        "x-kubernetes-list-type": "map",
        "x-kubernetes-list-map-keys": ["type"],
        "items": {
            "type": "object",
            "properties": {
                "lastTransitionTime": { "format": "date-time", "type": "string" },
                "message": { "type": "string" },
                "observedGeneration": { "type": "integer", "format": "int64", "default": 0 },
                "reason": { "type": "string" },
                "status": { "type": "string" },
                "type": { "type": "string" }
            },
            "required": [
                "lastTransitionTime",
                "message",
                "reason",
                "status",
                "type"
            ],
        },
    })
}

/// Defines a Result type for this module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    TracingSetup(Box<dyn std::error::Error + Send + Sync>),

    #[error(transparent)]
    Read(#[from] config::KubeconfigError),

    #[error("current context not set")]
    CurrentContextNotSet,

    #[error("current context not kind: {0}")]
    CurrentContextNotKind(String),
}

pub const MANAGER: &str = "rust-operator";
pub const MAX_HTML_BYTES: usize = 1_000_000;
pub const NAME_LABEL: &str = "app.kubernetes.io/name";
pub const INSTANCE_LABEL: &str = "app.kubernetes.io/instance";
pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
pub const FINALIZER_NAME: &str = "demo.ephemeral-roles.net/finalizer";

pub type ReconcileResult<T, E = ReconcileError> = std::result::Result<T, E>;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
pub enum ReconcileError {
    #[error("permanent error: {0}")]
    PermanentError(#[from] PermanentError),

    #[error("transient error: {0}")]
    TransientError(#[from] TransientError),
}

#[derive(thiserror::Error, Debug)]
pub enum PermanentError {
    #[error("controller owner reference not set")]
    EmptyControllerOwnerReference,

    #[error("namespace not set")]
    EmptyNamespace,

    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum TransientError {
    #[error("error getting namespace: {0}")]
    GetNamespace(#[source] kube::Error),

    #[error("error getting deployment: {0}")]
    GetDeployment(#[source] kube::Error),

    #[error("error getting webpage: {0}")]
    GetWebpage(#[source] kube::Error),

    #[error("error applying {kind} to {target}: {source}")]
    Patch {
        kind: String,
        target: String,
        #[source]
        source: Box<kube::Error>,
    },

    #[error("error applying {kind} status to {target}: {source}")]
    PatchStatus {
        kind: String,
        target: String,
        #[source]
        source: Box<kube::Error>,
    },

    #[error("error publishing event: {0}")]
    PublishEvent(#[source] kube::Error),
}

pub struct Context {
    pub recorder: Recorder,
    pub client: Client,
}

/// Sets global tracing subscriber with defaults.
///
/// # Errors
///
/// Returns an `Err` if the global tracing subscriber could not be set.
pub fn init_tracing_default(json: bool) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .flatten_event(true)
            .try_init()
            .map_err(Error::TracingSetup)
    } else {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(filter)
            .with_file(true)
            .with_line_number(true)
            .try_init()
            .map_err(Error::TracingSetup)
    }
}

#[must_use]
pub fn managed_by_label_selector() -> String {
    format!("{MANAGED_BY_LABEL}={MANAGER}")
}

/// Returns a Kube config if current context is set to a kind cluster.
///
/// # Errors
///
/// Returns an `Err` if the file cannot be read, current context is not set, or
/// if the current context does not start with 'kind-'.
pub fn kind_cluster_guard() -> Result<config::Kubeconfig> {
    let config = config::Kubeconfig::read()?;
    match &config.current_context {
        Some(ctx) => {
            if !ctx.starts_with("kind-") {
                return Err(Error::CurrentContextNotKind(ctx.clone()));
            }

            Ok(config)
        }
        None => Err(Error::CurrentContextNotSet),
    }
}

const WP_NAMESPACE_PREFIX: &str = "wp";

const PUBLIC_GATEWAY_LABEL: &str = "ephemeral-roles.net/public-gateway";

#[must_use]
pub fn new_namespace(name: &str, controller_ref: &OwnerReference) -> Namespace {
    let mut labels = new_match_labels(name);
    labels.insert(PUBLIC_GATEWAY_LABEL.to_string(), "true".to_string());
    labels.insert("linkerd.io/inject".to_string(), "enabled".to_string());

    Namespace {
        metadata: ObjectMeta {
            name: Some(format!("{WP_NAMESPACE_PREFIX}-{name}")),
            labels: Some(labels),
            owner_references: Some(vec![controller_ref.clone()]),
            ..Default::default()
        },
        spec: Some(NamespaceSpec::default()),
        ..Default::default()
    }
}

#[must_use]
pub fn new_object_meta(name: &str, controller_ref: &OwnerReference) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        labels: Some(new_match_labels(name)),
        owner_references: Some(vec![controller_ref.clone()]),
        ..Default::default()
    }
}

const INDEX_FILE: &str = "index.html";

#[must_use]
pub fn new_config_map(wp: &Webpage, name: &str, controller_ref: &OwnerReference) -> ConfigMap {
    ConfigMap {
        metadata: new_object_meta(name, controller_ref),
        data: Some(BTreeMap::from([(
            INDEX_FILE.to_string(),
            wp.spec.html.clone(),
        )])),
        ..Default::default()
    }
}

const PORT_NAME: &str = "http";
const PORT_NUMBER: i32 = 80;

#[must_use]
pub fn new_deployment(wp: &Webpage, name: &str, controller_ref: &OwnerReference) -> Deployment {
    Deployment {
        metadata: new_object_meta(name, controller_ref),
        spec: DeploymentSpec {
            replicas: Some(wp.spec.replicas),
            selector: LabelSelector {
                match_labels: Some(new_match_labels(name)),
                match_expressions: None,
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(new_match_labels(name).clone()),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: name.to_string(),
                        image: Some("nginx".to_string()),
                        ports: Some(vec![ContainerPort {
                            name: Some(PORT_NAME.to_string()),
                            container_port: PORT_NUMBER,
                            ..Default::default()
                        }]),
                        volume_mounts: Some(vec![VolumeMount {
                            name: name.to_string(),
                            mount_path: DEFAULT_FILE_LOCATION.to_string(),
                            read_only: Some(true),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    volumes: Some(vec![Volume {
                        name: name.to_string(),
                        config_map: Some(ConfigMapVolumeSource {
                            name: name.to_string(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

const DEFAULT_FILE_LOCATION: &str = "/usr/share/nginx/html";

#[must_use]
pub fn new_service(name: &str, controller_ref: &OwnerReference) -> Service {
    Service {
        metadata: new_object_meta(name, controller_ref),
        spec: Some(ServiceSpec {
            selector: Some(new_match_labels(name)),
            ports: Some(vec![ServicePort {
                name: Some(PORT_NAME.to_string()),
                port: PORT_NUMBER,
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[must_use]
pub fn new_http_route(name: &str, controller_ref: &OwnerReference) -> HttpRoute {
    let parent_ref = HttpRouteParentRefs {
        name: "public".to_string(),
        namespace: Some("envoy-gateway-system".to_string()),
        section_name: Some("https-wildcard".to_string()),
        ..Default::default()
    };

    let rule_match = HttpRouteRulesMatches {
        path: Some(HttpRouteRulesMatchesPath {
            r#type: Some(HttpRouteRulesMatchesPathType::PathPrefix),
            value: Some("/".to_string()),
        }),
        ..Default::default()
    };

    let backend_ref = HttpRouteRulesBackendRefs {
        kind: Some("Service".to_string()),
        name: name.to_string(),
        port: Some(PORT_NUMBER),
        ..Default::default()
    };

    let rules = HttpRouteRules {
        matches: Some(vec![rule_match]),
        backend_refs: Some(vec![backend_ref]),
        ..Default::default()
    };

    HttpRoute {
        metadata: new_object_meta(name, controller_ref),
        spec: HttpRouteSpec {
            hostnames: Some(vec![format!("{name}.ephemeral-roles.net")]),
            parent_refs: Some(vec![parent_ref]),
            rules: Some(vec![rules]),
        },
        ..Default::default()
    }
}

const CONDITION_STATUS_UNKNOWN: &str = "Unknown";
const CONDITION_STATUS_TRUE: &str = "True";
const CONDITION_STATUS_FALSE: &str = "False";
pub const CONDITION_REASON_HTML_TOO_LARGE: &str = "HtmlTooLarge";
const CONDITION_REASON_WAITING: &str = "Waiting";
const CONDITION_REASON_PROGRESSING: &str = "Progressing";
const CONDITION_REASON_READY: &str = "Ready";
pub const CONDITION_TYPE: &str = "Ready";

/// Returns the `Ready` condition from a status, if it has one.
#[must_use]
pub fn ready_condition(wp_status: &WebpageStatus) -> Option<&Condition> {
    wp_status
        .conditions
        .iter()
        .find(|c| c.type_ == CONDITION_TYPE)
}

#[must_use]
pub fn new_status(
    wp: &Webpage,
    wp_status: &WebpageStatus,
    dep_status: &DeploymentStatus,
) -> WebpageStatus {
    let (status, reason) = match (
        dep_status.observed_generation,
        dep_status.ready_replicas.unwrap_or_default(),
    ) {
        (None, _) => (CONDITION_STATUS_UNKNOWN, CONDITION_REASON_WAITING),
        (_, r) if r == wp.spec.replicas => (CONDITION_STATUS_TRUE, CONDITION_REASON_READY),
        _ => (CONDITION_STATUS_FALSE, CONDITION_REASON_PROGRESSING),
    };

    WebpageStatus {
        ready_replicas: dep_status.ready_replicas.unwrap_or_default(),
        observed_generation: wp.metadata.generation,
        conditions: vec![new_condition(
            wp.metadata.generation,
            ready_condition(wp_status),
            status.to_string(),
            reason.to_string(),
            format!("https://{}.ephemeral-roles.net", wp.name_any()),
        )],
    }
}

#[must_use]
pub fn new_too_large_status(wp: &Webpage, wp_status: &WebpageStatus) -> WebpageStatus {
    WebpageStatus {
        ready_replicas: 0,
        observed_generation: wp.metadata.generation,
        conditions: vec![new_condition(
            wp.metadata.generation,
            ready_condition(wp_status),
            CONDITION_STATUS_FALSE.to_string(),
            CONDITION_REASON_HTML_TOO_LARGE.to_string(),
            format!("html is {} bytes, max {MAX_HTML_BYTES}", wp.spec.html.len()),
        )],
    }
}

fn new_condition(
    observed_generation: Option<i64>,
    old: Option<&Condition>,
    status: String,
    reason: String,
    message: String,
) -> Condition {
    Condition {
        type_: CONDITION_TYPE.to_string(),
        // only move the timestamp when status flips
        last_transition_time: old.filter(|c| c.status == status).map_or_else(
            || Time(k8s_openapi::jiff::Timestamp::now()),
            |c| c.last_transition_time.clone(),
        ),
        observed_generation,
        status,
        reason,
        message,
    }
}

#[must_use]
fn new_match_labels(name: &str) -> BTreeMap<String, String> {
    let kind = Webpage::kind(&());
    BTreeMap::from([
        (NAME_LABEL.to_string(), kind.to_ascii_lowercase()),
        (
            INSTANCE_LABEL.to_string(),
            format!("{}-{name}", kind.to_ascii_lowercase()),
        ),
        (MANAGED_BY_LABEL.to_string(), MANAGER.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::{CustomResourceExt, Resource};

    const NAME: &str = "test-name";
    const EXPECTED_NAMESPACE: &str = const_format::concatcp!(WP_NAMESPACE_PREFIX, "-", NAME);
    const UID: &str = "test-uid";
    const EXPECTED_HTML: &str = "<!doctype html><html></html>";
    const EXPECTED_REPLICAS: i32 = 2;
    const GENERATION: i64 = 3;

    #[test]
    fn test_crd_spec_change() {
        let new = serde_json::to_value(Webpage::crd()).unwrap();
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open("manifests/crd.json").unwrap()).unwrap();

        pretty_assertions::assert_eq!(new, current, "crd changed. run `make crd` to regenerate.");
    }

    #[test]
    fn test_new_namespace() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let namespace = new_namespace(NAME, &controller_ref);
        assert_eq!(namespace.metadata.name.unwrap(), EXPECTED_NAMESPACE);

        let labels = namespace.metadata.labels.unwrap();
        assert_eq!(*labels.get(PUBLIC_GATEWAY_LABEL).unwrap(), true.to_string());

        assert!(namespace.spec.is_some());
    }

    #[test]
    fn test_new_object_meta() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let om = new_object_meta(NAME, &owner_ref);
        assert_eq!(om.name.unwrap(), NAME);
        assert_eq!(om.labels.unwrap(), new_match_labels(NAME));
        assert_eq!(om.owner_references.unwrap(), vec![owner_ref]);
    }

    #[test]
    fn test_new_config_map() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let cm = new_config_map(&wp, NAME, &owner_ref);
        assert_eq!(cm.data.unwrap().get(INDEX_FILE).unwrap(), EXPECTED_HTML);
    }

    #[test]
    fn test_new_deployment() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let dep = new_deployment(&wp, NAME, &owner_ref);
        assert_eq!(dep.spec.replicas.unwrap(), EXPECTED_REPLICAS);
    }

    #[test]
    fn test_new_service() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let service = new_service(NAME, &owner_ref);

        let spec = service.spec.unwrap();
        let selector = spec.selector.unwrap();

        let dep = new_deployment(&wp, NAME, &controller_ref);
        let pod_labels = dep.spec.template.metadata.unwrap().labels.unwrap();

        assert_eq!(selector, pod_labels);

        let service_ports = spec.ports.unwrap();
        assert_eq!(service_ports.len(), 1);

        let service_port = service_ports.into_iter().next().unwrap();
        assert_eq!(service_port.name.unwrap(), PORT_NAME);
        assert_eq!(service_port.port, PORT_NUMBER);
    }

    #[test]
    fn test_new_http_route() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);

        let http_route = new_http_route(NAME, &owner_ref);
        assert_eq!(http_route.metadata.name.unwrap(), NAME);

        let spec = http_route.spec;
        assert_eq!(
            spec.hostnames.unwrap(),
            vec![format!("{NAME}.ephemeral-roles.net")]
        );

        let parent_refs = spec.parent_refs.unwrap();
        assert_eq!(parent_refs.len(), 1);

        let parent_ref = parent_refs.into_iter().next().unwrap();
        assert_eq!(parent_ref.name, "public");
        assert_eq!(parent_ref.namespace.unwrap(), "envoy-gateway-system");
        assert_eq!(parent_ref.section_name.unwrap(), "https-wildcard");

        let rules = spec.rules.unwrap();
        assert_eq!(rules.len(), 1);

        let rule = rules.into_iter().next().unwrap();
        let path = rule
            .matches
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .path
            .unwrap();
        assert_eq!(
            path.r#type.unwrap(),
            HttpRouteRulesMatchesPathType::PathPrefix
        );
        assert_eq!(path.value.unwrap(), "/");

        let backend_refs = rule.backend_refs.unwrap();
        assert_eq!(backend_refs.len(), 1);

        let backend_ref = backend_refs.into_iter().next().unwrap();
        assert_eq!(backend_ref.kind.unwrap(), "Service");
        assert_eq!(backend_ref.name, NAME);
        assert_eq!(backend_ref.port.unwrap(), PORT_NUMBER);
    }

    #[test]
    fn test_new_status() {
        let (mut wp, _) = new_webpage_controller_ref();
        wp.metadata.generation = Some(GENERATION);
        let cases = [
            (
                None,
                None,
                CONDITION_STATUS_UNKNOWN,
                CONDITION_REASON_WAITING,
            ),
            (
                Some(1),
                Some(EXPECTED_REPLICAS),
                CONDITION_STATUS_TRUE,
                CONDITION_REASON_READY,
            ),
            (
                Some(1),
                Some(1),
                CONDITION_STATUS_FALSE,
                CONDITION_REASON_PROGRESSING,
            ),
            (
                Some(1),
                None,
                CONDITION_STATUS_FALSE,
                CONDITION_REASON_PROGRESSING,
            ),
        ];
        for (observed_generation, ready_replicas, status, reason) in cases {
            let dep_status = new_deployment_status(observed_generation, ready_replicas);
            let wp_status = new_status(&wp, &WebpageStatus::default(), &dep_status);
            assert_eq!(wp_status.ready_replicas, ready_replicas.unwrap_or_default());
            assert_eq!(wp_status.observed_generation, Some(GENERATION));
            assert_eq!(wp_status.conditions.len(), 1);
            let condition = &wp_status.conditions[0];
            assert_eq!(condition.type_, CONDITION_TYPE);
            assert_eq!(condition.status, status);
            assert_eq!(condition.reason, reason);
            assert_eq!(condition.observed_generation, Some(GENERATION));
            assert_eq!(
                condition.message,
                format!("https://{NAME}.ephemeral-roles.net")
            );
        }
    }

    #[test]
    fn test_new_too_large_status() {
        let (mut wp, _) = new_webpage_controller_ref();
        wp.metadata.generation = Some(GENERATION);

        let status = new_too_large_status(&wp, &WebpageStatus::default());
        assert_eq!(status.conditions[0].status, CONDITION_STATUS_FALSE);
        assert_eq!(status.conditions[0].reason, CONDITION_REASON_HTML_TOO_LARGE);
    }

    #[test]
    fn test_new_condition() {
        let (mut wp, _) = new_webpage_controller_ref();
        wp.metadata.generation = Some(GENERATION);

        // the transition time is kept while the status is unchanged and moves when it flips
        let old_time = Time(k8s_openapi::jiff::Timestamp::UNIX_EPOCH);
        let old = new_condition(
            wp.metadata.generation,
            None,
            CONDITION_STATUS_FALSE.to_string(),
            String::new(),
            String::new(),
        );
        let old = Condition {
            last_transition_time: old_time.clone(),
            ..old
        };

        let same = new_condition(
            wp.metadata.generation,
            Some(&old),
            CONDITION_STATUS_FALSE.to_string(),
            String::new(),
            String::new(),
        );
        assert_eq!(same.last_transition_time, old_time);

        let flipped = new_condition(
            wp.metadata.generation,
            Some(&old),
            CONDITION_STATUS_TRUE.to_string(),
            String::new(),
            String::new(),
        );
        assert_ne!(flipped.last_transition_time, old_time);
    }

    fn new_deployment_status(
        observed_generation: Option<i64>,
        ready_replicas: Option<i32>,
    ) -> DeploymentStatus {
        DeploymentStatus {
            observed_generation,
            ready_replicas,
            ..Default::default()
        }
    }

    fn new_webpage_controller_ref() -> (Webpage, OwnerReference) {
        let mut wp = Webpage::new(
            NAME,
            WebpageSpec {
                html: EXPECTED_HTML.to_string(),
                replicas: EXPECTED_REPLICAS,
            },
        );
        wp.metadata.uid = Some(UID.to_string());

        let controller_ref = wp
            .controller_owner_ref(&())
            .ok_or(PermanentError::EmptyControllerOwnerReference)
            .unwrap();

        (wp, controller_ref)
    }

    fn new_namespace_controller_ref(
        name: &str,
        controller_ref: &OwnerReference,
    ) -> (Namespace, OwnerReference) {
        let mut ns = new_namespace(name, controller_ref);
        ns.metadata.uid = Some(UID.to_string());

        (
            ns.clone(),
            ns.controller_owner_ref(&())
                .ok_or(PermanentError::EmptyControllerOwnerReference)
                .unwrap_or_default(),
        )
    }
}
