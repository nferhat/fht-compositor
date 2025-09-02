//! A single monitor.
//
//! This monitor is a representation of an [`Output`] for the workspace system to associate with it
//! [`Workspace`]s that contain [`Window`]s, etc...

use std::rc::Rc;
use std::time::Duration;

use fht_compositor_config::WorkspaceSwitchAnimationDirection;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::output::Output;
use smithay::utils::Point;

use super::workspace::{Workspace, WorkspaceRenderElement};
use super::Config;
use crate::fht_render_elements;
use crate::output::OutputExt;
use crate::renderer::FhtRenderer;
use crate::window::Window;

const WORKSPACE_COUNT: usize = 9;

pub struct Monitor {
    /// The output associated with the monitor.
    output: Output,
    /// The associated workspaces with the monitor.
    pub workspaces: [Workspace; WORKSPACE_COUNT],
    /// The active workspace index.
    pub active_idx: usize,
    /// Whether this monitor is the focused monitor.
    ///
    /// This should be updated in [`Monitor::refresh`].
    is_active: bool,
    /// Shared configuration with across the workspace system.
    pub config: Rc<Config>,
}

pub struct MonitorRenderResult<R: FhtRenderer> {
    /// The elements rendered from this result.
    pub elements: Vec<MonitorRenderElement<R>>,
    /// Whether we should render the monitor layer above the top Layer shells
    pub render_above_top: bool,
}

fht_render_elements! {
    MonitorRenderElement<R> => {
        Workspace = WorkspaceRenderElement<R>,
        SwitchingWorkspace = RelocateRenderElement<WorkspaceRenderElement<R>>,
    }
}

impl Monitor {
    /// Create a new [`Monitor`].
    pub fn new(output: Output, config: Rc<Config>) -> Self {
        // SAFETY: The length of our vector is always confirmed to be WORKSPACE_COUNT
        let workspaces = (0..WORKSPACE_COUNT)
            .map(|index| {
                let output = output.clone();
                Workspace::new(output, index, &config)
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();

        Self {
            output,
            workspaces,
            active_idx: 0,
            is_active: false,
            config,
        }
    }

    /// Run periodic update/clean-up tasks
    pub fn refresh(&mut self, is_active: bool) {
        crate::profile_function!();
        self.is_active = is_active;
        for workspace in &mut self.workspaces {
            workspace.refresh();
        }
    }

    /// Merge this [`Monitor`] with another one.
    pub fn merge_with(&mut self, other: Self) {
        assert!(
            self.output != other.output,
            "tried to merge a monitor with itself!"
        );

        for (workspace, other_workspace) in self.workspaces_mut().zip(other.workspaces) {
            workspace.merge_with(other_workspace);
        }
    }

    /// Get a reference to this [`Monitor`]'s associated [`Output`].
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Get whether this monitor is active
    pub fn active(&self) -> bool {
        self.is_active
    }

    /// Get an iterator over the monitor's [`Workspace`]s.
    pub fn workspaces(&self) -> impl ExactSizeIterator<Item = &Workspace> {
        self.workspaces.iter()
    }

    /// Get a mutable iterator over the monitor's [`Workspace`]s.
    pub fn workspaces_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Workspace> {
        self.workspaces.iter_mut()
    }

    /// Get a reference to a workspace [`Workspace`] from this [`Monitor`] by index.
    pub fn workspace_by_index(&self, index: usize) -> &Workspace {
        &self.workspaces[index]
    }

    /// Get a mutable reference to a workspace [`Workspace`] from this [`Monitor`] by index.
    pub fn workspace_mut_by_index(&mut self, index: usize) -> &mut Workspace {
        &mut self.workspaces[index]
    }

    /// Get the all the visible [`Window`] on this output.
    pub fn visible_windows(&self) -> impl ExactSizeIterator<Item = &Window> {
        self.workspaces[self.active_idx].windows()
    }

    /// Set the active [`Workspace`] index.
    pub fn set_active_workspace_idx(&mut self, idx: usize, animate: bool) -> Option<Window> {
        if self.active_idx == idx {
            return None;
        }

        // The workspace switch animation is done on a per-workspace level.
        // Each workspace has a render offset.
        if animate {
            if let Some((config, direction)) = &self.config.workspace_switch_animation {
                let (width, height) = self.output.geometry().size.into();
                match direction {
                    WorkspaceSwitchAnimationDirection::Horizontal => {
                        if self.active_idx > idx {
                            self.workspaces[self.active_idx].start_render_offset_animation(
                                Point::default(),
                                (width, 0).into(),
                                config,
                            );
                            self.workspaces[idx].start_render_offset_animation(
                                (-width, 0).into(),
                                Point::default(),
                                config,
                            );
                        } else {
                            self.workspaces[self.active_idx].start_render_offset_animation(
                                Point::default(),
                                (-width, 0).into(),
                                config,
                            );
                            self.workspaces[idx].start_render_offset_animation(
                                (width, 0).into(),
                                Point::default(),
                                config,
                            );
                        }
                    }
                    WorkspaceSwitchAnimationDirection::Vertical => {
                        if self.active_idx > idx {
                            self.workspaces[self.active_idx].start_render_offset_animation(
                                Point::default(),
                                (0, height).into(),
                                config,
                            );
                            self.workspaces[idx].start_render_offset_animation(
                                (0, -height).into(),
                                Point::default(),
                                config,
                            );
                        } else {
                            self.workspaces[self.active_idx].start_render_offset_animation(
                                Point::default(),
                                (0, -height).into(),
                                config,
                            );
                            self.workspaces[idx].start_render_offset_animation(
                                (0, height).into(),
                                Point::default(),
                                config,
                            );
                        }
                    }
                };
            }
        }

        self.active_idx = idx;

        self.workspaces[self.active_idx].active_window()
    }

    /// Get a reference to the active [`Workspace`].
    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_idx]
    }

    /// Get a the the active [`Workspace`] index.
    pub fn active_workspace_idx(&self) -> usize {
        self.active_idx
    }

    /// Get a reference to the active [`Workspace`].
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_idx]
    }

    /// Advance animations for this [`Monitor`].
    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        crate::profile_function!();
        self.workspaces.iter_mut().fold(false, |acc, ws| {
            ws.advance_animations(target_presentation_time) || acc
        })
    }

    /// Returns whether this monitor has any blurred regions.
    pub fn has_blur(&self) -> bool {
        for workspace in &self.workspaces {
            // only check for visible workspaces
            if (workspace.index() == self.active_idx || workspace.render_offset().is_some())
                && workspace.tiles().any(|tile| tile.has_transparent_region())
            {
                return true;
            }
        }

        false
    }

    /// Return whether the monitor contents should be rendered above the top layer shells
    pub fn render_above_top(&self) -> bool {
        let ws = self.active_workspace();
        ws.fullscreened_tile().is_some() && !ws.render_offset().is_some()
    }

    /// Create the render elements for this [`Monitor`]
    pub fn render<R: FhtRenderer>(&self, renderer: &mut R, scale: i32) -> MonitorRenderResult<R> {
        crate::profile_function!();
        // We want to render workspaces that currently have a render offset animation
        // as they could be displayed on the monitor (well this depends, but most of the time, yes)
        let mut elements = vec![];
        let mut render_above_top = false;

        for (idx, workspace) in self.workspaces.iter().enumerate() {
            if idx == self.active_idx || workspace.render_offset().is_some() {
                if !workspace.render_offset().is_some() {
                    // We only take this into account this when the workspace view is not currently
                    // animated/moved around.
                    render_above_top |= workspace.fullscreened_tile().is_some();
                }

                let render_offset = workspace.render_offset();
                let ws_elements = workspace
                    .render(renderer, scale, None)
                    .into_iter()
                    .map(Into::into);

                if let Some(render_offset) = render_offset {
                    let render_offset = render_offset.to_physical(scale);
                    elements.extend(
                        ws_elements
                            .map(|e| {
                                RelocateRenderElement::from_element(
                                    e,
                                    render_offset,
                                    Relocate::Relative,
                                )
                            })
                            .map(Into::into),
                    )
                } else {
                    elements.extend(ws_elements.map(Into::into))
                }

                continue;
            }
        }

        MonitorRenderResult {
            elements,
            render_above_top,
        }
    }
}
