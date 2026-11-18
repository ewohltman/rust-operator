use gateway_api::httproutes::{
    HttpRoute, HttpRouteParentRefs, HttpRouteRules, HttpRouteRulesBackendRefs,
    HttpRouteRulesMatches, HttpRouteRulesMatchesPath, HttpRouteRulesMatchesPathType, HttpRouteSpec,
};
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, Namespace, NamespaceSpec, PodSpec,
    PodTemplateSpec, Service, ServicePort, ServiceSpec, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta, OwnerReference};
use kube::config;
use kube::runtime::controller::Action;
use kube::{Client, CustomResource};
use kube::{KubeSchema, Resource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

/// A static web page served by nginx.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, KubeSchema, Eq, PartialEq)]
#[kube(
    group = "demo.ephemeral-roles.net",
    version = "v1alpha1",
    kind = "Webpage",
    namespaced
)]
#[kube(status = "WebpageStatus", shortname = "wp", doc = "An HTML webpage.")]
#[serde(rename_all = "camelCase")]
pub struct WebpageSpec {
    /// The HTML document to serve as `index.html`.
    #[schemars(length(max = 102_400))]
    #[x_kube(validation = Rule::new("self.lowerAscii().startsWith('<!doctype html>')").message("not a full HTML document")
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
#[derive(Deserialize, Serialize, Clone, Debug, Default, KubeSchema, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebpageStatus {
    /// Ready pods, as reported by the Deployment.
    pub ready_replicas: i32,
    /// The `metadata.generation` this status was computed from.
    pub observed_generation: Option<i64>,
}

/// Defines a Result type for this module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    TracingSetup(#[from] tracing::subscriber::SetGlobalDefaultError),

    #[error(transparent)]
    Read(#[from] config::KubeconfigError),

    #[error("current context not set")]
    CurrentContextNotSet,

    #[error("current context not kind: {0}")]
    CurrentContextNotKind(String),
}

/// Define a unique DNS-style finalizer name.
pub const FINALIZER_NAME: &str = "demo.ephemeral-roles.net/finalizer";

pub const NAME_LABEL: &str = "app.kubernetes.io/name";

pub const INSTANCE_LABEL: &str = "app.kubernetes.io/instance";

pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";

pub const MANAGER: &str = "rust-operator";

pub type ReconcileResult<T, E = ReconcileError> = std::result::Result<T, E>;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
pub enum ReconcileError {
    #[error("namespace not set")]
    EmptyNamespace,

    #[error("desired namespace name not set")]
    EmptyDesiredNamespace,

    #[error("controller owner reference not set")]
    EmptyControllerOwnerReference,

    #[error(transparent)]
    Api(#[from] kube::Error),

    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

pub struct Context {
    pub client: Client,
}

/// Sets global tracing subscriber with defaults.
///
/// # Errors
///
/// Returns an `Err` if the global tracing subscriber could not be set.
pub fn init_tracing_default() -> Result<()> {
    init_tracing(new_default_tracing_subscriber())
}

#[must_use]
pub fn managed_by_label_selector() -> String {
    format!("{MANAGED_BY_LABEL}={MANAGER}")
}

#[tracing::instrument(skip(_wp, _ctx))]
pub fn error_policy(_wp: Arc<Webpage>, err: &ReconcileError, _ctx: Arc<Context>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

/// Returns a Kube config if current context is set to a kind cluster.
///
/// # Errors
///
/// Returns an `Err` if the file cannot be read, current context is not set, or
/// if the current context does not start with 'kind-'.
#[tracing::instrument]
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
pub fn desired_namespace(name: &str, controller_ref: &OwnerReference) -> Namespace {
    let mut labels = match_labels(name);
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
pub fn desired_object_meta(name: &str, controller_ref: &OwnerReference) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        labels: Some(match_labels(name)),
        owner_references: Some(vec![controller_ref.clone()]),
        ..Default::default()
    }
}

const INDEX_FILE: &str = "index.html";

#[must_use]
pub fn desired_config_map(wp: &Webpage, name: &str, controller_ref: &OwnerReference) -> ConfigMap {
    ConfigMap {
        metadata: desired_object_meta(name, controller_ref),
        data: Some(BTreeMap::from([(
            INDEX_FILE.to_string(),
            wp.spec.html.clone(),
        )])),
        ..Default::default()
    }
}

#[must_use]
pub fn desired_deployment(wp: &Webpage, name: &str, controller_ref: &OwnerReference) -> Deployment {
    Deployment {
        metadata: desired_object_meta(name, controller_ref),
        spec: DeploymentSpec {
            replicas: Some(wp.spec.replicas),
            selector: LabelSelector {
                match_labels: Some(match_labels(name)),
                match_expressions: None,
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(match_labels(name).clone()),
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
pub fn desired_service(name: &str, controller_ref: &OwnerReference) -> Service {
    Service {
        metadata: desired_object_meta(name, controller_ref),
        spec: Some(ServiceSpec {
            selector: Some(match_labels(name)),
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
pub fn desired_http_route(name: &str, controller_ref: &OwnerReference) -> HttpRoute {
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
        metadata: desired_object_meta(name, controller_ref),
        spec: HttpRouteSpec {
            hostnames: Some(vec![format!("{name}.ephemeral-roles.net")]),
            parent_refs: Some(vec![parent_ref]),
            rules: Some(vec![rules]),
        },
        ..Default::default()
    }
}

#[must_use]
pub fn desired_status(wp: &Webpage) -> WebpageStatus {
    WebpageStatus {
        observed_generation: wp.metadata.generation,
        ready_replicas: wp.spec.replicas,
    }
}

const PORT_NAME: &str = "http";
const PORT_NUMBER: i32 = 80;

#[cfg(test)]
mod desired_tests {
    use super::*;

    const NAME: &str = "test-name";
    const EXPECTED_NAMESPACE: &str = const_format::concatcp!(WP_NAMESPACE_PREFIX, "-", NAME);
    const UID: &str = "test-uid";
    const EXPECTED_HTML: &str = "<!doctype html><html></html>";
    const EXPECTED_REPLICAS: i32 = 2;

    #[test]
    fn test_desired_namespace() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let namespace = desired_namespace(NAME, &controller_ref);
        assert_eq!(namespace.metadata.name.unwrap(), EXPECTED_NAMESPACE);

        let labels = namespace.metadata.labels.unwrap();
        assert_eq!(*labels.get(PUBLIC_GATEWAY_LABEL).unwrap(), true.to_string());

        assert!(namespace.spec.is_some());
    }

    #[test]
    fn test_desired_object_meta() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let om = desired_object_meta(NAME, &owner_ref);
        assert_eq!(om.name.unwrap(), NAME);
        assert_eq!(om.labels.unwrap(), match_labels(NAME));
        assert_eq!(om.owner_references.unwrap(), vec![owner_ref]);
    }

    #[test]
    fn test_desired_config_map() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let cm = desired_config_map(&wp, NAME, &owner_ref);
        assert_eq!(cm.data.unwrap().get(INDEX_FILE).unwrap(), EXPECTED_HTML);
    }

    #[test]
    fn test_desired_deployment() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let dep = desired_deployment(&wp, NAME, &owner_ref);
        assert_eq!(dep.spec.replicas.unwrap(), EXPECTED_REPLICAS);
    }

    #[test]
    fn test_desired_service() {
        let (wp, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);
        let service = desired_service(NAME, &owner_ref);

        let spec = service.spec.unwrap();
        let selector = spec.selector.unwrap();

        let dep = desired_deployment(&wp, NAME, &controller_ref);
        let pod_labels = dep.spec.template.metadata.unwrap().labels.unwrap();

        assert_eq!(selector, pod_labels);

        let service_ports = spec.ports.unwrap();
        assert_eq!(service_ports.len(), 1);

        let service_port = service_ports.into_iter().next().unwrap();
        assert_eq!(service_port.name.unwrap(), PORT_NAME);
        assert_eq!(service_port.port, PORT_NUMBER);
    }

    #[test]
    fn test_desired_status() {
        let (wp, _) = new_webpage_controller_ref();

        let status = desired_status(&wp);
        assert_eq!(status.ready_replicas, wp.spec.replicas);
        assert_eq!(status.observed_generation, wp.metadata.generation);
    }

    #[test]
    fn test_desired_http_route() {
        let (_, controller_ref) = new_webpage_controller_ref();
        let (_, owner_ref) = new_namespace_controller_ref(NAME, &controller_ref);

        let http_route = desired_http_route(NAME, &owner_ref);
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
            .ok_or(ReconcileError::EmptyControllerOwnerReference)
            .unwrap();

        (wp, controller_ref)
    }

    fn new_namespace_controller_ref(
        name: &str,
        controller_ref: &OwnerReference,
    ) -> (Namespace, OwnerReference) {
        let mut ns = desired_namespace(name, controller_ref);
        ns.metadata.uid = Some(UID.to_string());

        (
            ns.clone(),
            ns.controller_owner_ref(&())
                .ok_or(ReconcileError::EmptyControllerOwnerReference)
                .unwrap_or_default(),
        )
    }
}

/// Sets global tracing subscriber with the given subscriber.
///
/// # Errors
///
/// Returns an `Err` if the global tracing subscriber could not be set.
fn init_tracing<S>(subscriber: S) -> Result<()>
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    Ok(tracing::subscriber::set_global_default(subscriber)?)
}

/// Returns a new tracing subscriber with standardized config.
#[must_use]
fn new_default_tracing_subscriber() -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish()
}

#[must_use]
fn match_labels(name: &str) -> BTreeMap<String, String> {
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
    use kube::core::CustomResourceExt;

    #[test]
    fn test_crd_spec_change() {
        let new = serde_json::to_value(Webpage::crd()).unwrap();
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open("manifests/crd.json").unwrap()).unwrap();

        pretty_assertions::assert_eq!(new, current, "crd changed. run `make crd` to regenerate.");
    }
}
