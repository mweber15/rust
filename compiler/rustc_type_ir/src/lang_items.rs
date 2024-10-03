/// Lang items used by the new trait solver. This can be mapped to whatever internal
/// representation of `LangItem`s used in the underlying compiler implementation.
pub enum TraitSolverLangItem {
    // tidy-alphabetical-start
    AsyncDestruct,
    AsyncFn,
    AsyncFnKindHelper,
    AsyncFnKindUpvars,
    AsyncFnMut,
    AsyncFnOnce,
    AsyncFnOnceOutput,
    AsyncIterator,
    CallOnceFuture,
    CallRefFuture,
    Clone,
    Copy,
    Coroutine,
    CoroutineReturn,
    CoroutineYield,
    Destruct,
    DiscriminantKind,
    DynMetadata,
    EffectsCompat,
    EffectsIntersection,
    EffectsIntersectionOutput,
    EffectsMaybe,
    EffectsNoRuntime,
    EffectsRuntime,
    EffectsTyCompat,
    Fn,
    FnMut,
    FnOnce,
    FnPtrTrait,
    FusedIterator,
    Future,
    FutureOutput,
    Iterator,
    Metadata,
    Option,
    PointeeTrait,
    PointerLike,
    Poll,
    Sized,
    TransmuteTrait,
    Tuple,
    Unpin,
    Unsize,
    // tidy-alphabetical-end
}
