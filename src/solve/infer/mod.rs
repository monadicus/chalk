use ena::unify as ena;
use errors::*;
use ir::*;

mod canonicalize;
mod normalize_deep;
mod instantiate;
mod invert;
mod unify;
mod var;
#[cfg(test)] mod test;

pub use self::canonicalize::Canonicalized;
pub use self::unify::UnificationResult;
pub use self::var::{TyInferenceVariable, LifetimeInferenceVariable};
use self::var::*;

#[derive(Clone)]
pub struct InferenceTable {
    ty_unify: ena::UnificationTable<TyInferenceVariable>,
    ty_vars: Vec<TyInferenceVariable>,
    lifetime_unify: ena::UnificationTable<LifetimeInferenceVariable>,
    lifetime_vars: Vec<LifetimeInferenceVariable>,
}

pub struct InferenceSnapshot {
    ty_unify_snapshot: ena::Snapshot<TyInferenceVariable>,
    ty_vars: Vec<TyInferenceVariable>,
    lifetime_unify_snapshot: ena::Snapshot<LifetimeInferenceVariable>,
    lifetime_vars: Vec<LifetimeInferenceVariable>,
}

pub type ParameterInferenceVariable = ParameterKind<TyInferenceVariable, LifetimeInferenceVariable>;

impl InferenceTable {
    pub fn new() -> Self {
        InferenceTable {
            ty_unify: ena::UnificationTable::new(),
            ty_vars: vec![],
            lifetime_unify: ena::UnificationTable::new(),
            lifetime_vars: vec![],
        }
    }

    pub fn new_variable(&mut self, ui: UniverseIndex) -> TyInferenceVariable {
        let var = self.ty_unify.new_key(InferenceValue::Unbound(ui));
        self.ty_vars.push(var);
        var
    }

    pub fn new_lifetime_variable(&mut self, ui: UniverseIndex) -> LifetimeInferenceVariable {
        let var = self.lifetime_unify.new_key(InferenceValue::Unbound(ui));
        self.lifetime_vars.push(var);
        var
    }

    pub fn new_parameter_variable(&mut self, ui: ParameterKind<UniverseIndex>)
                                  -> ParameterInferenceVariable {
        match ui {
            ParameterKind::Ty(ui) => ParameterKind::Ty(self.new_variable(ui)),
            ParameterKind::Lifetime(ui) => ParameterKind::Lifetime(self.new_lifetime_variable(ui)),
        }
    }

    pub fn ty_vars(&self) -> &[TyInferenceVariable] {
        &self.ty_vars
    }

    pub fn lifetime_vars(&self) -> &[LifetimeInferenceVariable] {
        &self.lifetime_vars
    }

    pub fn snapshot(&mut self) -> InferenceSnapshot {
        let ty_unify_snapshot = self.ty_unify.snapshot();
        let lifetime_unify_snapshot = self.lifetime_unify.snapshot();
        let ty_vars = self.ty_vars.clone();
        let lifetime_vars = self.lifetime_vars.clone();
        InferenceSnapshot { ty_unify_snapshot, lifetime_unify_snapshot, ty_vars, lifetime_vars }
    }

    pub fn rollback_to(&mut self, snapshot: InferenceSnapshot) {
        self.ty_unify.rollback_to(snapshot.ty_unify_snapshot);
        self.lifetime_unify.rollback_to(snapshot.lifetime_unify_snapshot);
        self.ty_vars = snapshot.ty_vars;
        self.lifetime_vars = snapshot.lifetime_vars;
    }

    pub fn commit(&mut self, snapshot: InferenceSnapshot) {
        self.ty_unify.commit(snapshot.ty_unify_snapshot);
        self.lifetime_unify.commit(snapshot.lifetime_unify_snapshot);
    }

    pub fn commit_if_ok<F, R>(&mut self, op: F) -> Result<R>
        where F: FnOnce(&mut Self) -> Result<R>
    {
        let snapshot = self.snapshot();
        match op(self) {
            Ok(v) => {
                self.commit(snapshot);
                Ok(v)
            }

            Err(err) => {
                self.rollback_to(snapshot);
                Err(err)
            }
        }
    }

    /// If type `leaf` is a free inference variable, and that variable has been
    /// bound, returns `Some(T)` where `T` is the type to which it has been bound.
    ///
    /// `binders` is the number of binders under which `leaf` appears;
    /// the return value will also be shifted accordingly so that it
    /// can appear under that same number of binders.
    pub fn normalize_shallow(&mut self, leaf: &Ty, binders: usize) -> Option<Ty> {
        leaf.var()
            .and_then(|depth| {
                if depth < binders {
                    None // bound variable, not an inference var
                } else {
                    let var = TyInferenceVariable::from_depth(depth - binders);
                    match self.ty_unify.probe_value(var) {
                        InferenceValue::Unbound(_) => None,
                        InferenceValue::Bound(ref val) => Some(val.up_shift(binders)),
                    }
                }
            })
    }

    fn normalize_lifetime(&mut self, leaf: &Lifetime) -> Option<Lifetime> {
        match *leaf {
            Lifetime::Var(v) => self.probe_lifetime_var(LifetimeInferenceVariable::from_depth(v)),
            Lifetime::ForAll(_) => None,
        }
    }

    pub fn probe_var(&mut self, var: TyInferenceVariable) -> Option<Ty> {
        match self.ty_unify.probe_value(var) {
            InferenceValue::Unbound(_) => None,
            InferenceValue::Bound(ref val) => Some(val.clone()),
        }
    }

    pub fn probe_lifetime_var(&mut self, var: LifetimeInferenceVariable) -> Option<Lifetime> {
        match self.lifetime_unify.probe_value(var) {
            InferenceValue::Unbound(_) => None,
            InferenceValue::Bound(val) => Some(val.clone()),
        }
    }
}

impl Ty {
    /// If this is a `Ty::Var(d)`, returns `Some(d)` else `None`.
    pub fn var(&self) -> Option<usize> {
        if let Ty::Var(depth) = *self {
            Some(depth)
        } else {
            None
        }
    }

    /// If this is a `Ty::Var`, returns the
    /// `TyInferenceVariable` it represents. Only makes sense if
    /// `self` is known not to appear inside of any binders, since
    /// otherwise the depth would have be adjusted to account for
    /// those binders.
    pub fn inference_var(&self) -> Option<TyInferenceVariable> {
        self.var().map(TyInferenceVariable::from_depth)
    }
}

impl Lifetime {
    /// If this is a `Lifetime::Var(d)`, returns `Some(d)` else `None`.
    pub fn var(&self) -> Option<usize> {
        if let Lifetime::Var(depth) = *self {
            Some(depth)
        } else {
            None
        }
    }

    /// If this is a `Lifetime::Var`, returns the
    /// `LifetimeInferenceVariable` it represents. Only makes sense if
    /// `self` is known not to appear inside of any binders, since
    /// otherwise the depth would have be adjusted to account for
    /// those binders.
    pub fn inference_var(&self) -> Option<LifetimeInferenceVariable> {
        self.var().map(LifetimeInferenceVariable::from_depth)
    }
}

impl Substitution {
    /// Check whether this substitution is the identity substitution in the
    /// given inference context.
    pub fn is_trivial_within(&self, in_infer: &mut InferenceTable) -> bool {
        for ty in self.tys.values() {
            if let Some(var) = ty.inference_var() {
                if in_infer.probe_var(var).is_some() {
                    return false;
                }
            }
        }

        for lt in self.lifetimes.values() {
            if let Some(var) = lt.inference_var() {
                if in_infer.probe_lifetime_var(var).is_some() {
                    return false;
                }
            }
        }

        true
    }
}