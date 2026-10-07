//! Editable scene: parts, groups, anchors, and a motion timeline.
//!
//! This document stores links. It does not sample keyframes into frames.

use crate::error::{ProjectError, Result};
use crate::ids::{GroupId, MediaRefId, PartId, SceneId};
use reelforge_core::MediaTime;
use reelforge_render_graph::{Animated, CurveFault, Keyframe};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Current [`SceneDocument`] schema.
pub const SCENE_DOCUMENT_VERSION: u32 = 1;

/// A point in a part's local space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScenePoint {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

impl ScenePoint {
    /// Construct a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Picture (and optional mask) a part is cut from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLink {
    /// Library entry.
    pub media: MediaRefId,
    /// Optional mask asset id. Absent means the whole source frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
}

impl SourceLink {
    /// Link a media entry without a mask.
    #[must_use]
    pub fn media(id: impl Into<String>) -> Self {
        Self {
            media: MediaRefId::new(id),
            mask: None,
        }
    }

    /// Attach a mask asset id.
    #[must_use]
    pub fn with_mask(mut self, mask: impl Into<String>) -> Self {
        self.mask = Some(mask.into());
        self
    }
}

/// Attachment of one part onto another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartAnchor {
    /// Part this one is bound to.
    pub target: PartId,
    /// Point on the target, in the target's local space.
    pub point: ScenePoint,
}

/// One editable piece of the scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenePart {
    /// Id.
    pub id: PartId,
    /// Original source. A replacement does not erase it.
    pub source: SourceLink,
    /// Accepted substitute for [`Self::source`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<SourceLink>,
    /// Parent part. Motion of the parent carries this part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PartId>,
    /// Group membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
    /// Local pivot.
    pub pivot: ScenePoint,
    /// Binding onto another part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PartAnchor>,
    /// Overlap order. Higher draws above lower.
    #[serde(default)]
    pub order: i32,
    /// Hidden parts stay in the document and drop out of the picture.
    #[serde(default)]
    pub hidden: bool,
}

impl ScenePart {
    /// A visible part with no parent, group, or anchor.
    #[must_use]
    pub fn new(id: impl Into<String>, source: SourceLink) -> Self {
        Self {
            id: PartId::new(id),
            source,
            replacement: None,
            parent: None,
            group: None,
            pivot: ScenePoint::new(0.0, 0.0),
            anchor: None,
            order: 0,
            hidden: false,
        }
    }

    /// Source used for the picture: the replacement when one is set.
    #[must_use]
    pub fn effective_source(&self) -> &SourceLink {
        self.replacement.as_ref().unwrap_or(&self.source)
    }
}

/// Named set of parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneGroup {
    /// Id.
    pub id: GroupId,
    /// Display name.
    pub name: String,
    /// Member parts. The part's own `group` field is the authority.
    #[serde(default)]
    pub parts: Vec<PartId>,
}

impl SceneGroup {
    /// Empty group.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: GroupId::new(id),
            name: name.into(),
            parts: Vec::new(),
        }
    }
}

/// Which value a motion channel drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionProperty {
    /// Local horizontal position.
    PositionX,
    /// Local vertical position.
    PositionY,
    /// Rotation in degrees.
    Rotation,
    /// Uniform scale.
    Scale,
    /// Opacity from 0 to 1.
    Opacity,
}

impl MotionProperty {
    fn name(self) -> &'static str {
        match self {
            Self::PositionX => "position_x",
            Self::PositionY => "position_y",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
            Self::Opacity => "opacity",
        }
    }
}

/// One sample on a motion channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionKey {
    /// Time on the motion timeline.
    pub t: MediaTime,
    /// Value in the property's unit.
    pub value: f64,
}

/// Keys for one part and one property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionChannel {
    /// Driven part.
    pub part: PartId,
    /// Driven property.
    pub property: MotionProperty,
    /// Keys in authoring order.
    #[serde(default)]
    pub keys: Vec<MotionKey>,
}

/// Motion that belongs to the scene, separate from the edit timeline.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MotionTimeline {
    /// Channels.
    #[serde(default)]
    pub channels: Vec<MotionChannel>,
}

/// Saved editable scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDocument {
    /// Schema version. Missing values load as 0 and migrate to [`SCENE_DOCUMENT_VERSION`].
    #[serde(default)]
    pub version: u32,
    /// Id.
    pub id: SceneId,
    /// Display name.
    pub name: String,
    /// Parts.
    #[serde(default)]
    pub parts: Vec<ScenePart>,
    /// Groups.
    #[serde(default)]
    pub groups: Vec<SceneGroup>,
    /// Motion timeline.
    #[serde(default)]
    pub motion: MotionTimeline,
}

impl SceneDocument {
    /// Empty scene at the current schema version.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            version: SCENE_DOCUMENT_VERSION,
            id: SceneId::new(id),
            name: name.into(),
            parts: Vec::new(),
            groups: Vec::new(),
            motion: MotionTimeline::default(),
        }
    }

    /// Serialize.
    ///
    /// # Errors
    ///
    /// JSON encoding failure.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|err| ProjectError::message(err.to_string()))
    }

    /// Parse and migrate a scene document.
    ///
    /// # Errors
    ///
    /// Invalid JSON, or a version newer than [`SCENE_DOCUMENT_VERSION`].
    pub fn from_json(text: &str) -> Result<Self> {
        let mut doc: Self =
            serde_json::from_str(text).map_err(|err| ProjectError::message(err.to_string()))?;
        if doc.version > SCENE_DOCUMENT_VERSION {
            return Err(ProjectError::message(format!(
                "scene document version {} is not supported",
                doc.version
            )));
        }
        if doc.version == 0 {
            doc.version = SCENE_DOCUMENT_VERSION;
        }
        Ok(doc)
    }

    /// Borrow a part.
    ///
    /// # Errors
    ///
    /// Unknown part id.
    pub fn part(&self, id: &PartId) -> Result<&ScenePart> {
        let index = self.part_index(id)?;
        Ok(&self.parts[index])
    }

    /// Record a substitute source for one part. Other parts stay unchanged.
    ///
    /// # Errors
    ///
    /// Unknown part id.
    pub fn replace_source(&mut self, id: &PartId, source: SourceLink) -> Result<()> {
        let index = self.part_index(id)?;
        self.parts[index].replacement = Some(source);
        Ok(())
    }

    /// Hide or show one part.
    ///
    /// # Errors
    ///
    /// Unknown part id.
    pub fn set_hidden(&mut self, id: &PartId, hidden: bool) -> Result<()> {
        let index = self.part_index(id)?;
        self.parts[index].hidden = hidden;
        Ok(())
    }

    /// Bind one part to another. A part cannot anchor to itself.
    ///
    /// # Errors
    ///
    /// Unknown part, unknown target, or a self anchor.
    pub fn bind_anchor(&mut self, id: &PartId, anchor: PartAnchor) -> Result<()> {
        let index = self.part_index(id)?;
        if &anchor.target == id {
            return Err(ProjectError::message(format!(
                "part {} cannot anchor to itself",
                id.as_str()
            )));
        }
        self.part_index(&anchor.target)?;
        self.parts[index].anchor = Some(anchor);
        Ok(())
    }

    /// Put a part in a group and drop it from the previous group.
    ///
    /// # Errors
    ///
    /// Unknown part or unknown group.
    pub fn assign_group(&mut self, id: &PartId, group: &GroupId) -> Result<()> {
        let part_index = self.part_index(id)?;
        let group_index = self.group_index(group)?;
        if let Some(previous) = self.parts[part_index].group.clone()
            && let Some(old) = self.groups.iter_mut().find(|item| item.id == previous)
        {
            old.parts.retain(|member| member != id);
        }
        self.parts[part_index].group = Some(group.clone());
        if !self.groups[group_index]
            .parts
            .iter()
            .any(|member| member == id)
        {
            self.groups[group_index].parts.push(id.clone());
        }
        Ok(())
    }

    /// Append a motion key for one part. Other parts gain no channel.
    ///
    /// # Errors
    ///
    /// Unknown part id.
    pub fn add_motion_key(
        &mut self,
        id: &PartId,
        property: MotionProperty,
        key: MotionKey,
    ) -> Result<()> {
        self.part_index(id)?;
        if let Some(channel) = self
            .motion
            .channels
            .iter_mut()
            .find(|channel| &channel.part == id && channel.property == property)
        {
            channel.keys.push(key);
            return Ok(());
        }
        self.motion.channels.push(MotionChannel {
            part: id.clone(),
            property,
            keys: vec![key],
        });
        Ok(())
    }

    /// Admit this document for compilation.
    ///
    /// Links, groups, and motion keys are checked here. A compose curve fault is
    /// not treated as validation of the whole document.
    ///
    /// # Errors
    ///
    /// Duplicate ids, a broken parent or anchor link, a cycle, a group that
    /// disagrees with its parts, or a motion key that cannot be sampled.
    pub fn validate(&self) -> Result<()> {
        self.check_parts()?;
        self.check_groups()?;
        self.check_links(|part| part.parent.as_ref(), "parent")?;
        self.check_links(
            |part| part.anchor.as_ref().map(|anchor| &anchor.target),
            "anchor",
        )?;
        self.check_links(
            |part| {
                part.parent
                    .as_ref()
                    .or(part.anchor.as_ref().map(|anchor| &anchor.target))
            },
            "pose",
        )?;
        self.check_motion()?;
        Ok(())
    }

    fn check_parts(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for part in &self.parts {
            if part.id.as_str().is_empty() {
                return Err(ProjectError::message("part id is empty"));
            }
            if !ids.insert(part.id.clone()) {
                return Err(ProjectError::message(format!(
                    "duplicate part {}",
                    part.id.as_str()
                )));
            }
            if part.source.media.as_str().is_empty() {
                return Err(ProjectError::message(format!(
                    "part {} source media is empty",
                    part.id.as_str()
                )));
            }
            if part
                .replacement
                .as_ref()
                .is_some_and(|source| source.media.as_str().is_empty())
            {
                return Err(ProjectError::message(format!(
                    "part {} replacement media is empty",
                    part.id.as_str()
                )));
            }
            if !part.pivot.x.is_finite() || !part.pivot.y.is_finite() {
                return Err(ProjectError::message(format!(
                    "part {} pivot is not finite",
                    part.id.as_str()
                )));
            }
            if part.parent.is_some() && part.anchor.is_some() {
                return Err(ProjectError::message(format!(
                    "part {} has both a parent and an anchor",
                    part.id.as_str()
                )));
            }
        }
        Ok(())
    }

    fn check_groups(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for group in &self.groups {
            if group.id.as_str().is_empty() {
                return Err(ProjectError::message("group id is empty"));
            }
            if !ids.insert(group.id.clone()) {
                return Err(ProjectError::message(format!(
                    "duplicate group {}",
                    group.id.as_str()
                )));
            }
        }
        for part in &self.parts {
            if let Some(group) = &part.group
                && self.group_index(group).is_err()
            {
                return Err(ProjectError::message(format!(
                    "part {} group {} does not exist",
                    part.id.as_str(),
                    group.as_str()
                )));
            }
        }
        for group in &self.groups {
            let mut listed = BTreeSet::new();
            for member in &group.parts {
                if !listed.insert(member.clone()) {
                    return Err(ProjectError::message(format!(
                        "group {} lists part {} twice",
                        group.id.as_str(),
                        member.as_str()
                    )));
                }
                let Some(part) = self.parts.iter().find(|part| &part.id == member) else {
                    return Err(ProjectError::message(format!(
                        "group {} lists part {} that is not a member",
                        group.id.as_str(),
                        member.as_str()
                    )));
                };
                if part.group.as_ref() != Some(&group.id) {
                    return Err(ProjectError::message(format!(
                        "group {} lists part {} that is not a member",
                        group.id.as_str(),
                        member.as_str()
                    )));
                }
            }
            for part in &self.parts {
                if part.group.as_ref() == Some(&group.id) && !listed.contains(&part.id) {
                    return Err(ProjectError::message(format!(
                        "group {} is missing part {}",
                        group.id.as_str(),
                        part.id.as_str()
                    )));
                }
            }
        }
        Ok(())
    }

    fn check_links(&self, next: impl Fn(&ScenePart) -> Option<&PartId>, label: &str) -> Result<()> {
        for part in &self.parts {
            let Some(target) = next(part) else {
                continue;
            };
            if target == &part.id {
                return Err(ProjectError::message(format!(
                    "part {} {label}s itself",
                    part.id.as_str()
                )));
            }
            if self.part_index(target).is_err() {
                return Err(ProjectError::message(format!(
                    "part {} {label} {} does not exist",
                    part.id.as_str(),
                    target.as_str()
                )));
            }
        }
        let mut color = vec![0_u8; self.parts.len()];
        for start in 0..self.parts.len() {
            let mut cursor = Some(start);
            while let Some(index) = cursor {
                if color[index] == 2 {
                    break;
                }
                if color[index] == 1 {
                    return Err(ProjectError::message(format!(
                        "part {} is in a {label} cycle",
                        self.parts[index].id.as_str()
                    )));
                }
                color[index] = 1;
                cursor = next(&self.parts[index])
                    .and_then(|target| self.parts.iter().position(|part| &part.id == target));
            }
            let mut back = Some(start);
            while let Some(index) = back {
                if color[index] == 2 {
                    break;
                }
                color[index] = 2;
                back = next(&self.parts[index])
                    .and_then(|target| self.parts.iter().position(|part| &part.id == target));
            }
        }
        Ok(())
    }

    fn check_motion(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for channel in &self.motion.channels {
            if self.part_index(&channel.part).is_err() {
                return Err(ProjectError::message(format!(
                    "motion channel names part {} that does not exist",
                    channel.part.as_str()
                )));
            }
            if !seen.insert((channel.part.clone(), channel.property)) {
                return Err(ProjectError::message(format!(
                    "part {} has two {} channels",
                    channel.part.as_str(),
                    channel.property.name()
                )));
            }
            check_keys(channel)?;
        }
        Ok(())
    }

    fn part_index(&self, id: &PartId) -> Result<usize> {
        self.parts
            .iter()
            .position(|part| &part.id == id)
            .ok_or_else(|| ProjectError::message(format!("unknown part {}", id.as_str())))
    }

    fn group_index(&self, id: &GroupId) -> Result<usize> {
        self.groups
            .iter()
            .position(|group| &group.id == id)
            .ok_or_else(|| ProjectError::message(format!("unknown group {}", id.as_str())))
    }
}

fn check_keys(channel: &MotionChannel) -> Result<()> {
    let name = channel.property.name();
    let mut keys = Vec::with_capacity(channel.keys.len());
    for key in &channel.keys {
        if key.t.timescale == 0 {
            return Err(ProjectError::message(format!(
                "part {} {name} key has a zero timescale",
                channel.part.as_str()
            )));
        }
        if key.t.ticks < 0 {
            return Err(ProjectError::message(format!(
                "part {} {name} key is negative",
                channel.part.as_str()
            )));
        }
        let Some(value) = finite_f32(key.value) else {
            return Err(ProjectError::message(format!(
                "part {} {name} key is not finite",
                channel.part.as_str()
            )));
        };
        match channel.property {
            MotionProperty::Opacity if !(0.0..=1.0).contains(&key.value) => {
                return Err(ProjectError::message(format!(
                    "part {} opacity is outside 0..=1",
                    channel.part.as_str()
                )));
            }
            MotionProperty::Scale if key.value <= 0.0 => {
                return Err(ProjectError::message(format!(
                    "part {} scale is not positive",
                    channel.part.as_str()
                )));
            }
            _ => {}
        }
        keys.push(Keyframe::new(key.t, value));
    }
    let fault = Animated::keyframes(keys).curve_fault();
    let message = match fault {
        Some(CurveFault::Empty) => {
            format!("part {} {name} channel has no keys", channel.part.as_str())
        }
        Some(CurveFault::NotStrictlyIncreasing) => format!(
            "part {} {name} channel is not strictly increasing",
            channel.part.as_str()
        ),
        None => return Ok(()),
    };
    Err(ProjectError::message(message))
}

pub(crate) fn finite_f32(value: f64) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let narrowed = value as f32;
    narrowed.is_finite().then_some(narrowed)
}

pub(crate) fn motion_curve(
    scene: &SceneDocument,
    id: &PartId,
    property: MotionProperty,
    default: f32,
) -> Animated<f32> {
    let Some(channel) = scene
        .motion
        .channels
        .iter()
        .find(|channel| &channel.part == id && channel.property == property)
    else {
        return Animated::constant(default);
    };
    Animated::keyframes(
        channel
            .keys
            .iter()
            .filter_map(|key| finite_f32(key.value).map(|value| Keyframe::new(key.t, value)))
            .collect(),
    )
}
