use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WatchPartyLayout {
    #[default]
    ReactionsRight,
    ReactionStrip,
    PictureInPicture,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourcePlacement {
    pub destination: RectF,
    pub source_uv: RectF,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositionPlan {
    pub main: SourcePlacement,
    pub reaction: SourcePlacement,
}

pub fn composition_plan(
    layout: WatchPartyLayout,
    canvas_width: u32,
    canvas_height: u32,
    main_size: (u32, u32),
    reaction_size: (u32, u32),
) -> Result<CompositionPlan, String> {
    if canvas_width == 0
        || canvas_height == 0
        || main_size.0 == 0
        || main_size.1 == 0
        || reaction_size.0 == 0
        || reaction_size.1 == 0
    {
        return Err("Watch Party composition dimensions must be nonzero.".to_string());
    }
    let canvas_width = canvas_width as f32;
    let canvas_height = canvas_height as f32;
    let margin = (canvas_width * 0.01875).round().max(18.0);
    let (main_slot, reaction_slot) = match layout {
        WatchPartyLayout::ReactionsRight => {
            let reaction_width = (canvas_width * 0.28).round();
            (
                RectF {
                    x: 0.0,
                    y: 0.0,
                    width: canvas_width - reaction_width,
                    height: canvas_height,
                },
                RectF {
                    x: canvas_width - reaction_width,
                    y: 0.0,
                    width: reaction_width,
                    height: canvas_height,
                },
            )
        }
        WatchPartyLayout::ReactionStrip => {
            let reaction_height = (canvas_height * 0.25).round();
            (
                RectF {
                    x: 0.0,
                    y: 0.0,
                    width: canvas_width,
                    height: canvas_height - reaction_height,
                },
                RectF {
                    x: 0.0,
                    y: canvas_height - reaction_height,
                    width: canvas_width,
                    height: reaction_height,
                },
            )
        }
        WatchPartyLayout::PictureInPicture => {
            let reaction_width = (canvas_width * 0.30).round();
            let reaction_height = (reaction_width * 9.0 / 16.0).round();
            (
                RectF {
                    x: 0.0,
                    y: 0.0,
                    width: canvas_width,
                    height: canvas_height,
                },
                RectF {
                    x: canvas_width - reaction_width - margin,
                    y: canvas_height - reaction_height - margin,
                    width: reaction_width,
                    height: reaction_height,
                },
            )
        }
    };
    Ok(CompositionPlan {
        main: contain(main_slot, main_size),
        reaction: contain(reaction_slot, reaction_size),
    })
}

fn contain(slot: RectF, source: (u32, u32)) -> SourcePlacement {
    let scale = (slot.width / source.0 as f32).min(slot.height / source.1 as f32);
    let width = source.0 as f32 * scale;
    let height = source.1 as f32 * scale;
    SourcePlacement {
        destination: RectF {
            x: slot.x + (slot.width - width) * 0.5,
            y: slot.y + (slot.height - height) * 0.5,
            width,
            height,
        },
        source_uv: RectF {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(rect: RectF, width: f32, height: f32) -> bool {
        rect.x >= 0.0
            && rect.y >= 0.0
            && rect.x + rect.width <= width + 0.01
            && rect.y + rect.height <= height + 0.01
    }

    #[test]
    fn every_preset_stays_inside_the_canvas_and_preserves_aspect_ratio() {
        for layout in [
            WatchPartyLayout::ReactionsRight,
            WatchPartyLayout::ReactionStrip,
            WatchPartyLayout::PictureInPicture,
        ] {
            let plan = composition_plan(layout, 1920, 1080, (2560, 1440), (900, 1600)).unwrap();
            assert!(inside(plan.main.destination, 1920.0, 1080.0));
            assert!(inside(plan.reaction.destination, 1920.0, 1080.0));
            assert!(
                (plan.main.destination.width / plan.main.destination.height - 16.0 / 9.0).abs()
                    < 0.01
            );
            assert!(
                (plan.reaction.destination.width / plan.reaction.destination.height
                    - 900.0 / 1600.0)
                    .abs()
                    < 0.01
            );
        }
    }

    #[test]
    fn discord_resize_reflows_without_changing_the_selected_layout() {
        let before = composition_plan(
            WatchPartyLayout::ReactionsRight,
            1920,
            1080,
            (1920, 1080),
            (1280, 720),
        )
        .unwrap();
        let after = composition_plan(
            WatchPartyLayout::ReactionsRight,
            1920,
            1080,
            (1920, 1080),
            (720, 1280),
        )
        .unwrap();
        assert_eq!(before.main, after.main);
        assert_ne!(after.reaction.destination, before.reaction.destination);
        assert!(after.reaction.destination.height > before.reaction.destination.height);
        assert!(inside(after.reaction.destination, 1920.0, 1080.0));
    }

    #[test]
    fn rejects_zero_sized_sources() {
        assert!(composition_plan(
            WatchPartyLayout::PictureInPicture,
            1920,
            1080,
            (0, 1080),
            (1280, 720),
        )
        .is_err());
    }
}
