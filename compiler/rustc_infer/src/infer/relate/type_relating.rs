use super::combine::CombineFields;
use crate::infer::BoundRegionConversionTime::HigherRankedType;
use crate::infer::{
    DefineOpaqueTypes, ObligationEmittingRelation, StructurallyRelateAliases, SubregionOrigin,
};
use crate::traits::{Obligation, PredicateObligations};

use rustc_middle::ty::relate::{Relate, RelateResult, TypeRelation};
use rustc_middle::ty::TyVar;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::Span;

/// Enforce that `a` is equal to or a subtype of `b`.
pub struct TypeRelating<'combine, 'a, 'tcx> {
    fields: &'combine mut CombineFields<'a, 'tcx>,
    structurally_relate_aliases: StructurallyRelateAliases,
    ambient_variance: ty::Variance,
}

impl<'combine, 'infcx, 'tcx> TypeRelating<'combine, 'infcx, 'tcx> {
    pub fn new(
        f: &'combine mut CombineFields<'infcx, 'tcx>,
        structurally_relate_aliases: StructurallyRelateAliases,
        ambient_variance: ty::Variance,
    ) -> TypeRelating<'combine, 'infcx, 'tcx> {
        TypeRelating { fields: f, structurally_relate_aliases, ambient_variance }
    }
}

impl<'tcx> TypeRelation<'tcx> for TypeRelating<'_, '_, 'tcx> {
    fn tag(&self) -> &'static str {
        "TypeRelating"
    }

    fn tcx(&self) -> TyCtxt<'tcx> {
        self.fields.infcx.tcx
    }

    fn relate_with_variance<T: Relate<'tcx>>(
        &mut self,
        variance: ty::Variance,
        _info: ty::VarianceDiagInfo<'tcx>,
        a: T,
        b: T,
    ) -> RelateResult<'tcx, T> {
        let old_ambient_variance = self.ambient_variance;
        self.ambient_variance = self.ambient_variance.xform(variance);
        debug!(?self.ambient_variance, "new ambient variance");

        let r = if self.ambient_variance == ty::Bivariant { Ok(a) } else { self.relate(a, b) };

        self.ambient_variance = old_ambient_variance;
        r
    }

    #[instrument(skip(self), level = "debug")]
    fn tys(&mut self, a: Ty<'tcx>, b: Ty<'tcx>) -> RelateResult<'tcx, Ty<'tcx>> {
        if a == b {
            return Ok(a);
        }

        let infcx = self.fields.infcx;
        let a = infcx.inner.borrow_mut().type_variables().replace_if_possible(a);
        let b = infcx.inner.borrow_mut().type_variables().replace_if_possible(b);

        match (a.kind(), b.kind()) {
            (&ty::Infer(TyVar(a_id)), &ty::Infer(TyVar(b_id))) => {
                match self.ambient_variance {
                    ty::Covariant => {
                        // can't make progress on `A <: B` if both A and B are
                        // type variables, so record an obligation.
                        self.fields.obligations.push(Obligation::new(
                            self.tcx(),
                            self.fields.trace.cause.clone(),
                            self.fields.param_env,
                            ty::Binder::dummy(ty::PredicateKind::Subtype(ty::SubtypePredicate {
                                a_is_expected: true,
                                a,
                                b,
                            })),
                        ));
                    }
                    ty::Contravariant => {
                        // can't make progress on `B <: A` if both A and B are
                        // type variables, so record an obligation.
                        self.fields.obligations.push(Obligation::new(
                            self.tcx(),
                            self.fields.trace.cause.clone(),
                            self.fields.param_env,
                            ty::Binder::dummy(ty::PredicateKind::Subtype(ty::SubtypePredicate {
                                a_is_expected: false,
                                a: b,
                                b: a,
                            })),
                        ));
                    }
                    ty::Invariant => {
                        infcx.inner.borrow_mut().type_variables().equate(a_id, b_id);
                    }
                    ty::Bivariant => {
                        unreachable!("Expected bivariance to be handled in relate_with_variance")
                    }
                }
            }

            (&ty::Infer(TyVar(a_vid)), _) => {
                infcx.instantiate_ty_var(self, true, a_vid, self.ambient_variance, b)?;
            }
            (_, &ty::Infer(TyVar(b_vid))) => {
                infcx.instantiate_ty_var(
                    self,
                    false,
                    b_vid,
                    self.ambient_variance.xform(ty::Contravariant),
                    a,
                )?;
            }

            (&ty::Error(e), _) | (_, &ty::Error(e)) => {
                infcx.set_tainted_by_errors(e);
                return Ok(Ty::new_error(self.tcx(), e));
            }

            (
                &ty::Alias(ty::Opaque, ty::AliasTy { def_id: a_def_id, .. }),
                &ty::Alias(ty::Opaque, ty::AliasTy { def_id: b_def_id, .. }),
            ) if a_def_id == b_def_id => {
                infcx.super_combine_tys(self, a, b)?;
            }

            (&ty::Alias(ty::Opaque, ty::AliasTy { def_id, .. }), _)
            | (_, &ty::Alias(ty::Opaque, ty::AliasTy { def_id, .. }))
                if self.fields.define_opaque_types == DefineOpaqueTypes::Yes
                    && def_id.is_local()
                    && !infcx.next_trait_solver() =>
            {
                self.fields.obligations.extend(
                    infcx
                        .handle_opaque_type(a, b, &self.fields.trace.cause, self.param_env())?
                        .obligations,
                );
            }

            _ => {
                infcx.super_combine_tys(self, a, b)?;
            }
        }

        Ok(a)
    }

    fn regions(
        &mut self,
        a: ty::Region<'tcx>,
        b: ty::Region<'tcx>,
    ) -> RelateResult<'tcx, ty::Region<'tcx>> {
        debug!("{}.regions({:?}, {:?})", self.tag(), a, b);
        let origin = SubregionOrigin::Subtype(Box::new(self.fields.trace.clone()));

        match self.ambient_variance {
            // Subtype(&'a u8, &'b u8) => Outlives('a: 'b) => SubRegion('b, 'a)
            ty::Covariant => {
                self.fields
                    .infcx
                    .inner
                    .borrow_mut()
                    .unwrap_region_constraints()
                    .make_subregion(origin, b, a);
            }
            // Suptype(&'a u8, &'b u8) => Outlives('b: 'a) => SubRegion('a, 'b)
            ty::Contravariant => {
                self.fields
                    .infcx
                    .inner
                    .borrow_mut()
                    .unwrap_region_constraints()
                    .make_subregion(origin, a, b);
            }
            ty::Invariant => {
                self.fields
                    .infcx
                    .inner
                    .borrow_mut()
                    .unwrap_region_constraints()
                    .make_eqregion(origin, a, b);
            }
            ty::Bivariant => {
                unreachable!("Expected bivariance to be handled in relate_with_variance")
            }
        }

        Ok(a)
    }

    fn consts(
        &mut self,
        a: ty::Const<'tcx>,
        b: ty::Const<'tcx>,
    ) -> RelateResult<'tcx, ty::Const<'tcx>> {
        self.fields.infcx.super_combine_consts(self, a, b)
    }

    fn binders<T>(
        &mut self,
        a: ty::Binder<'tcx, T>,
        b: ty::Binder<'tcx, T>,
    ) -> RelateResult<'tcx, ty::Binder<'tcx, T>>
    where
        T: Relate<'tcx>,
    {
        if a == b {
            // Do nothing
        } else if let Some(a) = a.no_bound_vars()
            && let Some(b) = b.no_bound_vars()
        {
            self.relate(a, b)?;
        } else {
            let span = self.fields.trace.cause.span;
            let infcx = self.fields.infcx;

            match self.ambient_variance {
                // Checks whether `for<..> sub <: for<..> sup` holds.
                //
                // For this to hold, **all** instantiations of the super type
                // have to be a super type of **at least one** instantiation of
                // the subtype.
                //
                // This is implemented by first entering a new universe.
                // We then replace all bound variables in `sup` with placeholders,
                // and all bound variables in `sub` with inference vars.
                // We can then just relate the two resulting types as normal.
                //
                // Note: this is a subtle algorithm. For a full explanation, please see
                // the [rustc dev guide][rd]
                //
                // [rd]: https://rustc-dev-guide.rust-lang.org/borrow_check/region_inference/placeholders_and_universes.html
                ty::Covariant => {
                    infcx.enter_forall(b, |b| {
                        let a = infcx.instantiate_binder_with_fresh_vars(span, HigherRankedType, a);
                        self.relate(a, b)
                    })?;
                }
                ty::Contravariant => {
                    infcx.enter_forall(a, |a| {
                        let b = infcx.instantiate_binder_with_fresh_vars(span, HigherRankedType, b);
                        self.relate(a, b)
                    })?;
                }

                // When **equating** binders, we check that there is a 1-to-1
                // correspondence between the bound vars in both types.
                //
                // We do so by separately instantiating one of the binders with
                // placeholders and the other with inference variables and then
                // equating the instantiated types.
                //
                // We want `for<..> A == for<..> B` -- therefore we want
                // `exists<..> A == for<..> B` and `exists<..> B == for<..> A`.
                // Check if `exists<..> A == for<..> B`
                ty::Invariant => {
                    infcx.enter_forall(b, |b| {
                        let a = infcx.instantiate_binder_with_fresh_vars(span, HigherRankedType, a);
                        self.relate(a, b)
                    })?;

                    // Check if `exists<..> B == for<..> A`.
                    infcx.enter_forall(a, |a| {
                        let b = infcx.instantiate_binder_with_fresh_vars(span, HigherRankedType, b);
                        self.relate(a, b)
                    })?;
                }
                ty::Bivariant => {
                    unreachable!("Expected bivariance to be handled in relate_with_variance")
                }
            }
        }

        Ok(a)
    }
}

impl<'tcx> ObligationEmittingRelation<'tcx> for TypeRelating<'_, '_, 'tcx> {
    fn span(&self) -> Span {
        self.fields.trace.span()
    }

    fn param_env(&self) -> ty::ParamEnv<'tcx> {
        self.fields.param_env
    }

    fn structurally_relate_aliases(&self) -> StructurallyRelateAliases {
        self.structurally_relate_aliases
    }

    fn register_predicates(&mut self, obligations: impl IntoIterator<Item: ty::ToPredicate<'tcx>>) {
        self.fields.register_predicates(obligations);
    }

    fn register_obligations(&mut self, obligations: PredicateObligations<'tcx>) {
        self.fields.register_obligations(obligations);
    }

    fn register_type_relate_obligation(&mut self, a: Ty<'tcx>, b: Ty<'tcx>) {
        self.register_predicates([ty::Binder::dummy(match self.ambient_variance {
            ty::Variance::Covariant => ty::PredicateKind::AliasRelate(
                a.into(),
                b.into(),
                ty::AliasRelationDirection::Subtype,
            ),
            // a :> b is b <: a
            ty::Variance::Contravariant => ty::PredicateKind::AliasRelate(
                b.into(),
                a.into(),
                ty::AliasRelationDirection::Subtype,
            ),
            ty::Variance::Invariant => ty::PredicateKind::AliasRelate(
                a.into(),
                b.into(),
                ty::AliasRelationDirection::Equate,
            ),
            ty::Variance::Bivariant => {
                unreachable!("Expected bivariance to be handled in relate_with_variance")
            }
        })]);
    }
}
