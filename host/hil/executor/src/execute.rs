//! Running one scenario instance with one executor.

use core::{fmt, future::Future};

use oer_hil_model::{IdentityError, Instance, Operation, Sha256, Spec, Verdict};

use crate::{Executed, Executor, ExecutorKind, Recorder, RunsOn};

/// A failure of the stand, the link or the host, not of the subject. A
/// run that ends with one has no trace, and so no verdict
/// (`broken/infrastructure`).
#[derive(Debug)]
pub struct Infrastructure {
    /// What failed, such as `link.sync` or `target.report`.
    pub operation: Operation,
    pub cause: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for Infrastructure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation, self.cause)
    }
}

impl std::error::Error for Infrastructure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Execution of a scenario by executor `X`. A scenario implements it for
/// the executors whose driver it can use, typically generically over a
/// HIL trait of reports that the model agent (around the port's harness)
/// and the stand's proxy implement, so that the host model and the stand
/// run the same code; [`execute`] admits the pair only when the executor's kind
/// [runs](RunsOn) the scenario's purpose.
pub trait Run<X: Executor>: Spec {
    /// Drives the subject through `executor` and returns what it recorded.
    /// It decides nothing: the verdict is [`Spec::verdict`] over the trace.
    fn run(
        params: &Self::Params,
        executor: &mut X,
        recorder: &mut Recorder,
    ) -> impl Future<Output = Result<Self::Trace, Infrastructure>>;
}

/// One executed instance.
pub struct Outcome<S: Spec> {
    pub executed: Executed,
    pub identity: Sha256,
    pub trace: S::Trace,
    pub verdict: Verdict<S::Finding>,
    pub recorder: Recorder,
}

impl<S: Spec> fmt::Debug for Outcome<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outcome")
            .field("executed", &self.executed)
            .field("identity", &self.identity)
            .field("trace", &self.trace)
            .field("verdict", &self.verdict)
            .field("recorder", &self.recorder)
            .finish()
    }
}

/// Why an instance has no verdict.
#[derive(Debug)]
pub enum Cause {
    Infrastructure(Infrastructure),
    /// The scenario itself is defective: its parameters or criteria hold a
    /// NaN or an infinite number, or have no JSON form, so the instance has
    /// no identity.
    Identity(IdentityError),
}

/// An instance without a verdict, with what it counted before.
#[derive(Debug)]
pub struct Failed {
    pub executed: Executed,
    pub cause: Cause,
    pub recorder: Recorder,
}

/// Runs `instance` with `executor` and judges its trace with `criteria`.
pub async fn execute<S, X>(
    instance: &Instance<S>,
    criteria: &S::Criteria,
    executor: &mut X,
) -> Result<Outcome<S>, Failed>
where
    X: Executor,
    S: Run<X>,
    S::Purpose: RunsOn<X::Kind>,
{
    let executed = <X::Kind as ExecutorKind>::VALUE;
    let mut recorder = Recorder::default();
    let identity = match instance.identity(criteria) {
        Ok(identity) => identity,
        Err(error) => {
            return Err(Failed {
                executed,
                cause: Cause::Identity(error),
                recorder,
            });
        }
    };
    match S::run(&instance.params, executor, &mut recorder).await {
        Ok(trace) => {
            let verdict = S::verdict(&trace, criteria);
            Ok(Outcome {
                executed,
                identity,
                trace,
                verdict,
                recorder,
            })
        }
        Err(infrastructure) => Err(Failed {
            executed,
            cause: Cause::Infrastructure(infrastructure),
            recorder,
        }),
    }
}
