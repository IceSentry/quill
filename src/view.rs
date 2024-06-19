use crate::{cx::Cx, tracking_scope::TrackingScope, NodeSpan};
use bevy::{
    hierarchy::Parent,
    log::info,
    prelude::{Added, Component, Entity, With, World},
    utils::hashbrown::HashSet,
};
use std::sync::{Arc, Mutex};

#[allow(unused)]
/// An object which produces one or more display nodes. The `View` is itself immutable and
/// stateless, but it can produce a mutable state object which is updated when the view is rebuilt.
/// This state object must be managed externally, and is passed to the `View` methods as a
/// parameter.
///
/// Views also produce outputs in the form of display nodes, which are entities in the ECS world.
/// These can be Bevy UI elements, effects, or other entities that are part of the view hierarchy.
pub trait View: Sync + Send + 'static {
    /// The external state for this View.
    type State: Send + Sync;

    /// Return the span of entities produced by this View.
    fn nodes(&self, world: &World, state: &Self::State) -> NodeSpan;

    /// Construct and patch the tree of UiNodes produced by this view.
    /// This may also spawn child entities representing nested components.
    fn build(&self, cx: &mut Cx) -> Self::State;

    /// Update the internal state of this view, re-creating any UiNodes.
    /// Returns true if the output changed, that is, if `nodes()` would return a different value
    /// than it did before the rebuild.
    fn rebuild(&self, cx: &mut Cx, state: &mut Self::State) -> bool;

    /// Instructs the view to attach any child entities to their parent entity. This is called
    /// whenever we know that one or more child entities have changed their outputs. It also
    /// does the same thing recursively for any child views of this view, but only within
    /// the current template.
    ///
    /// This function normally returns false, which means that there is nothing more to be done.
    /// However, some view implementations are just thin wrappers around other views, in which
    /// case they should return true to indicate that the parent of this view should also re-attach
    /// its children.
    fn attach_children(&self, world: &mut World, state: &mut Self::State) -> bool {
        false
    }

    /// Recursively despawn any child entities that were created as a result of calling `.build()`.
    /// This calls `.raze()` for any nested views within the current view state.
    fn raze(&self, world: &mut World, state: &mut Self::State);

    // / Build a ViewRoot from this view.
    fn to_root(self) -> (ViewStateCell<Self>, ViewThunk, ViewRoot)
    where
        Self: Sized,
    {
        let holder = ViewStateCell::new(self);
        let thunk = holder.create_thunk();
        (holder, thunk, ViewRoot)
    }
}

/// Marker on a [`View`] entity to indicate that it's output [`NodeSpan`] has changed, and that
/// the parent needs to re-attach it's children.
#[derive(Component)]
pub struct OutputChanged;

/// Combination of a [`View`] and it's built state, stored as a trait object within a component.
pub struct ViewState<V: View> {
    pub(crate) view: V,
    pub(crate) state: Option<V::State>,
}

impl<V: View> ViewState<V> {
    fn rebuild(&mut self, cx: &mut Cx) -> bool {
        if let Some(state) = self.state.as_mut() {
            self.view.rebuild(cx, state)
        } else {
            let state = self.view.build(cx);
            self.state = Some(state);
            true
        }
    }

    fn raze(&mut self, world: &mut World) {
        if let Some(state) = self.state.as_mut() {
            self.view.raze(world, state);
        }
    }

    fn attach_children(&mut self, world: &mut World) -> bool {
        if let Some(state) = self.state.as_mut() {
            self.view.attach_children(world, state)
        } else {
            false
        }
    }
}

#[derive(Component)]
pub struct ViewStateCell<V: View>(pub Arc<Mutex<ViewState<V>>>);

impl<V: View> ViewStateCell<V> {
    pub fn new(view: V) -> Self {
        Self(Arc::new(Mutex::new(ViewState { view, state: None })))
    }

    pub fn create_thunk(&self) -> ViewThunk {
        ViewThunk(&ViewAdapter::<V> {
            marker: std::marker::PhantomData,
        })
    }
}

pub struct ViewAdapter<V: View> {
    marker: std::marker::PhantomData<V>,
}

/// Type-erased trait for a [`ViewState`].
pub trait AnyViewAdapter: Sync + Send + 'static {
    /// Return the span of entities produced by this View.
    fn nodes(&self, world: &mut World, entity: Entity) -> NodeSpan;

    /// Update the internal state of this view, re-creating any UiNodes. Returns true if the output
    /// changed, that is, if `nodes()` would return a different value than it did before the
    /// rebuild.
    fn rebuild(&self, world: &mut World, entity: Entity, scope: &mut TrackingScope) -> bool;

    /// Recursively despawn any child entities that were created as a result of calling `.build()`.
    /// This calls `.raze()` for any nested views within the current view state.
    fn raze(&self, world: &mut World, entity: Entity);

    /// Instructs the view to attach any child entities to the parent entity. This is called
    /// whenever we know that one or more child entities have changed.
    fn attach_children(&self, world: &mut World, entity: Entity) -> bool;
}

impl<V: View> AnyViewAdapter for ViewAdapter<V> {
    fn nodes(&self, world: &mut World, entity: Entity) -> NodeSpan {
        match world.entity(entity).get::<ViewStateCell<V>>() {
            Some(view_cell) => {
                let vstate = view_cell.0.lock().unwrap();
                match &vstate.state {
                    Some(state) => vstate.view.nodes(world, state),
                    None => NodeSpan::Empty,
                }
            }
            None => NodeSpan::Empty,
        }
    }

    fn rebuild(&self, world: &mut World, entity: Entity, scope: &mut TrackingScope) -> bool {
        let mut cx = Cx::new(world, entity, scope);
        if let Some(view_cell) = cx
            .world_mut()
            .entity_mut(entity)
            .get_mut::<ViewStateCell<V>>()
        {
            let inner = view_cell.0.clone();
            let mut vstate = inner.lock().unwrap();
            vstate.rebuild(&mut cx)
        } else {
            false
        }
    }

    fn raze(&self, world: &mut World, entity: Entity) {
        if let Some(vsh) = world.entity_mut(entity).take::<ViewStateCell<V>>() {
            vsh.0.lock().unwrap().raze(world);
        }
    }

    fn attach_children(&self, world: &mut World, entity: Entity) -> bool {
        if let Some(view_cell) = world.entity(entity).get::<ViewStateCell<V>>() {
            let vs = view_cell.0.clone();
            let mut inner = vs.lock().unwrap();
            inner.attach_children(world)
        } else {
            false
        }
    }
}

#[derive(Component)]
pub struct ViewThunk(pub(crate) &'static dyn AnyViewAdapter);

/// An ECS component which holds a reference to the root of a view hierarchy.
#[derive(Component)]
pub struct ViewRoot;

/// A reference to a [`View`] which can be passed around as a parameter.
// pub struct ViewHandle(pub(crate) Arc<Mutex<dyn AnyViewState>>);

/// View which renders nothing.
pub struct EmptyView;

//     fn raze(&self, world: &mut World, state: &mut Self::State) {
//         let vc = world.entity(state.0).get::<ViewCell>().unwrap();
//         let cell = vc.0.clone();
//         let mut view = cell.lock().unwrap();
//         view.raze(world);
//         world.entity_mut(state.0).remove_parent();
//         world.entity_mut(state.0).despawn();
//     }
// }

pub(crate) fn build_views(world: &mut World) {
    let mut roots = world.query_filtered::<(Entity, &ViewThunk), Added<ViewRoot>>();
    let roots_copy: Vec<Entity> = roots.iter(world).map(|(e, _)| e).collect();
    let tick = world.change_tick();
    for root_entity in roots_copy.iter() {
        let Ok((_, root)) = roots.get(world, *root_entity) else {
            continue;
        };
        let mut scope = TrackingScope::new(tick);
        root.0.rebuild(world, *root_entity, &mut scope);
        world.entity_mut(*root_entity).insert(scope);
    }
}

pub(crate) fn rebuild_views(world: &mut World) {
    // let mut divergence_ct: usize = 0;
    // let mut prev_change_ct: usize = 0;
    let this_run = world.change_tick();

    // let mut v = HashSet::new();

    // Scan changed resources
    let mut scopes = world.query::<(Entity, &mut TrackingScope, &ViewThunk)>();
    let changed = scopes
        .iter(world)
        .filter_map(|(e, scope, _)| {
            if scope.dependencies_changed(world, this_run) {
                Some(e)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    // if !changed.is_empty() {
    //     println!("# Changed views: {:?}", changed.len());
    // }
    // for (e, scope) in q.iter(world) {
    //     if scope.dependencies_changed(world, this_run) {
    //         v.insert(e);
    //     }
    // }

    // Record the changed entities for debugging purposes.
    // if let Some(mut tracing) = world.get_resource_mut::<TrackingScopeTracing>() {
    //     // Check for empty first to avoid setting mutation flag.
    //     if !tracing.0.is_empty() {
    //         tracing.0.clear();
    //     }
    //     if !changed.is_empty() {
    //         tracing.0.extend(changed.iter().copied());
    //     }
    // }

    for scope_entity in changed.iter() {
        // println!("Rebuilding view {:?}", scope_entity);
        // Call registered cleanup functions
        let (_, mut scope, _) = scopes.get_mut(world, *scope_entity).unwrap();
        let mut cleanups = std::mem::take(&mut scope.cleanups);
        for cleanup_fn in cleanups.drain(..) {
            cleanup_fn(world);
        }

        // Run the reaction
        let (_, _, view_cell) = scopes.get_mut(world, *scope_entity).unwrap();
        let mut next_scope = TrackingScope::new(this_run);
        let output_changed = view_cell.0.rebuild(world, *scope_entity, &mut next_scope);
        if output_changed {
            #[cfg(feature = "verbose")]
            info!("View output changed: {}", *scope_entity);
            world.entity_mut(*scope_entity).insert(OutputChanged);
        }

        // Replace deps and cleanups in the current scope with the next scope.
        let (_, mut scope, _) = scopes.get_mut(world, *scope_entity).unwrap();
        scope.take_deps(&mut next_scope);
        scope.tick = this_run;
    }

    // // force build every view that just got spawned
    // let mut qf = world.query_filtered::<Entity, Added<ViewHandle>>();
    // for e in qf.iter(world) {
    //     v.insert(e);
    // }

    // loop {
    //     // This is inside a loop because rendering may trigger further changes.

    //     // This means that either a presenter was just added, or its props got modified by a parent.
    //     let mut qf =
    //         world.query_filtered::<Entity, (With<ViewHandle>, With<PresenterStateChanged>)>();
    //     for e in qf.iter_mut(world) {
    //         v.insert(e);
    //     }

    //     for e in v.iter() {
    //         world.entity_mut(*e).remove::<PresenterStateChanged>();
    //     }

    //     // Most of the time changes will converge, that is, the number of changed presenters
    //     // decreases each time through the loop. A "divergence" is when that fails to happen.
    //     // We tolerate a maximum number of divergences before giving up.
    //     let change_ct = v.len();
    //     if change_ct >= prev_change_ct {
    //         divergence_ct += 1;
    //         if divergence_ct > MAX_DIVERGENCE_CT {
    //             panic!("Reactions failed to converge, num changes: {}", change_ct);
    //         }
    //     }
    //     prev_change_ct = change_ct;

    // let mut child_nodes_changed =
    //     world.query_filtered::<(Entity, &mut TrackingScope, &ViewThunk), With<ChildNodesChanged>>();
    // let changed = child_nodes_changed
    //     .iter(world)
    //     .map(|(e, _, _)| e)
    //     .collect::<Vec<_>>();
    // for e in changed.iter() {
    //     #[cfg(feature = "verbose")]
    //     info!("Child node change detected: {}", *e);

    //     let (_, _, thunk) = child_nodes_changed.get_mut(world, *e).unwrap();
    //     thunk.0.attach_children(world, *e);
    //     world.entity_mut(*e).remove::<ChildNodesChanged>();
    // }
}

pub(crate) fn reattach_children(world: &mut World) {
    let mut changed_views = Vec::<Entity>::new();
    let mut work_queue = HashSet::<Entity>::new();
    let mut changed_views_query = world.query_filtered::<Entity, With<OutputChanged>>();
    for view_entity in changed_views_query.iter(world) {
        changed_views.push(view_entity);
        if let Some(parent) = world.entity(view_entity).get::<Parent>() {
            work_queue.insert(parent.get());
        }
    }

    for view_entity in changed_views.drain(..) {
        world.entity_mut(view_entity).remove::<OutputChanged>();
    }

    while !work_queue.is_empty() {
        let entity = *work_queue.iter().next().unwrap();
        work_queue.remove(&entity);

        if let Some(thunk) = world.entity(entity).get::<ViewThunk>() {
            if thunk.0.attach_children(world, entity) {
                if let Some(parent) = world.entity(entity).get::<Parent>() {
                    work_queue.insert(parent.get());
                }
            }
        }

        if work_queue.is_empty() {
            break;
        }
    }
}
