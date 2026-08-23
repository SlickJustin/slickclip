use std::collections::{HashMap, VecDeque};

use super::compositor::CpuFrame;
use super::layout::{contain, layout_slots, RectF, SourcePlacement, WatchPartyLayout};

const GRID_WIDTH: usize = 48;
const MIN_COMPONENT_CELLS: usize = 12;
const REQUIRED_STABLE_OBSERVATIONS: u8 = 3;
const MAX_MISSED_OBSERVATIONS: u8 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct ParticipantDetection {
    pub crops: Vec<RectF>,
    pub confidence: f32,
}

#[derive(Default)]
pub struct ParticipantTracker {
    candidate: Option<ParticipantDetection>,
    candidate_observations: u8,
    current: Option<ParticipantDetection>,
    misses: u8,
}

impl ParticipantTracker {
    pub fn update(&mut self, frame: &CpuFrame) -> Option<&ParticipantDetection> {
        match detect_participants(frame) {
            Some(detection) => {
                self.misses = 0;
                if self
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| similar(candidate, &detection))
                {
                    self.candidate_observations = self.candidate_observations.saturating_add(1);
                } else {
                    self.candidate = Some(detection);
                    self.candidate_observations = 1;
                }
                if self.candidate_observations >= REQUIRED_STABLE_OBSERVATIONS {
                    self.current = self.candidate.clone();
                }
            }
            None => {
                self.candidate = None;
                self.candidate_observations = 0;
                self.misses = self.misses.saturating_add(1);
                if self.misses >= MAX_MISSED_OBSERVATIONS {
                    self.current = None;
                }
            }
        }
        self.current.as_ref()
    }

    pub fn current(&self) -> Option<&ParticipantDetection> {
        self.current.as_ref()
    }
}

pub fn participant_placements(
    layout: WatchPartyLayout,
    canvas_width: u32,
    canvas_height: u32,
    source_size: (u32, u32),
    detection: &ParticipantDetection,
) -> Vec<SourcePlacement> {
    let (_, reaction_slot) = layout_slots(layout, canvas_width, canvas_height);
    let cells = target_cells(reaction_slot, detection.crops.len());
    cells
        .into_iter()
        .zip(&detection.crops)
        .map(|(cell, crop)| {
            let crop_size = (
                (crop.width * source_size.0 as f32).max(1.0) as u32,
                (crop.height * source_size.1 as f32).max(1.0) as u32,
            );
            let mut placement = contain(inset(cell, 4.0), crop_size);
            placement.source_uv = *crop;
            placement
        })
        .collect()
}

pub fn detect_participants(frame: &CpuFrame) -> Option<ParticipantDetection> {
    if frame.width < 160
        || frame.height < 90
        || frame.pixels.len() != frame.width as usize * frame.height as usize * 4
    {
        return None;
    }
    let grid_height = ((GRID_WIDTH as f32 * frame.height as f32 / frame.width as f32).round()
        as usize)
        .clamp(18, 48);
    let background = estimate_background(frame);
    let mut active = vec![false; GRID_WIDTH * grid_height];
    for gy in 0..grid_height {
        for gx in 0..GRID_WIDTH {
            active[gy * GRID_WIDTH + gx] =
                block_foreground_fraction(frame, background, gx, gy, GRID_WIDTH, grid_height)
                    >= 0.28;
        }
    }
    close_single_cell_gaps(&mut active, GRID_WIDTH, grid_height);
    let mut components = connected_components(&active, GRID_WIDTH, grid_height)
        .into_iter()
        .filter(|component| component.cells >= MIN_COMPONENT_CELLS)
        .filter_map(|component| component.to_crop(GRID_WIDTH, grid_height))
        .filter(|rect| rect.width * rect.height >= 0.055)
        .collect::<Vec<_>>();
    if !(2..=4).contains(&components.len()) {
        return None;
    }
    components.sort_by(|left, right| {
        left.y
            .total_cmp(&right.y)
            .then_with(|| left.x.total_cmp(&right.x))
    });
    let areas = components
        .iter()
        .map(|rect| rect.width * rect.height)
        .collect::<Vec<_>>();
    let smallest = areas.iter().copied().fold(f32::MAX, f32::min);
    let largest = areas.iter().copied().fold(0.0, f32::max);
    let coverage = areas.iter().sum::<f32>();
    if smallest <= 0.0 || largest / smallest > 2.8 || coverage < 0.32 {
        return None;
    }
    let balance = (smallest / largest).clamp(0.0, 1.0);
    Some(ParticipantDetection {
        crops: components,
        confidence: (0.55 + coverage.min(0.8) * 0.3 + balance * 0.15).min(0.99),
    })
}

#[derive(Clone, Copy)]
struct Component {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
    cells: usize,
}

impl Component {
    fn to_crop(self, width: usize, height: usize) -> Option<RectF> {
        let padding_x = 1.0 / width as f32;
        let padding_y = 1.0 / height as f32;
        let x = self.min_x as f32 / width as f32;
        let y = self.min_y as f32 / height as f32;
        let right = (self.max_x + 1) as f32 / width as f32;
        let bottom = (self.max_y + 1) as f32 / height as f32;
        let rect = RectF {
            x: (x - padding_x).max(0.0),
            y: (y - padding_y).max(0.0),
            width: (right - x + padding_x * 2.0).min(1.0 - x),
            height: (bottom - y + padding_y * 2.0).min(1.0 - y),
        };
        (rect.width > 0.0 && rect.height > 0.0).then_some(rect)
    }
}

fn estimate_background(frame: &CpuFrame) -> [u8; 3] {
    let mut colors = HashMap::<[u8; 3], usize>::new();
    let step_x = (frame.width / 64).max(1);
    let step_y = (frame.height / 36).max(1);
    for x in (0..frame.width).step_by(step_x as usize) {
        for y in [0, frame.height - 1] {
            *colors.entry(quantized_pixel(frame, x, y)).or_default() += 1;
        }
    }
    for y in (0..frame.height).step_by(step_y as usize) {
        for x in [0, frame.width - 1] {
            *colors.entry(quantized_pixel(frame, x, y)).or_default() += 1;
        }
    }
    colors
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(color, _)| color)
        .unwrap_or([48, 48, 48])
}

fn quantized_pixel(frame: &CpuFrame, x: u32, y: u32) -> [u8; 3] {
    let index = (y as usize * frame.width as usize + x as usize) * 4;
    [
        frame.pixels[index] & 0xF0,
        frame.pixels[index + 1] & 0xF0,
        frame.pixels[index + 2] & 0xF0,
    ]
}

fn block_foreground_fraction(
    frame: &CpuFrame,
    background: [u8; 3],
    gx: usize,
    gy: usize,
    grid_width: usize,
    grid_height: usize,
) -> f32 {
    let start_x = gx as u32 * frame.width / grid_width as u32;
    let end_x = ((gx + 1) as u32 * frame.width / grid_width as u32).max(start_x + 1);
    let start_y = gy as u32 * frame.height / grid_height as u32;
    let end_y = ((gy + 1) as u32 * frame.height / grid_height as u32).max(start_y + 1);
    let step_x = ((end_x - start_x) / 4).max(1);
    let step_y = ((end_y - start_y) / 4).max(1);
    let mut foreground = 0u32;
    let mut samples = 0u32;
    for y in (start_y..end_y.min(frame.height)).step_by(step_y as usize) {
        for x in (start_x..end_x.min(frame.width)).step_by(step_x as usize) {
            let pixel = quantized_pixel(frame, x, y);
            let distance = pixel
                .iter()
                .zip(background)
                .map(|(value, background)| i32::from(*value) - i32::from(background))
                .map(|delta| delta * delta)
                .sum::<i32>();
            foreground += u32::from(distance > 1_200);
            samples += 1;
        }
    }
    foreground as f32 / samples.max(1) as f32
}

fn close_single_cell_gaps(active: &mut [bool], width: usize, height: usize) {
    let original = active.to_vec();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y * width + x;
            if !original[index] {
                let neighbors = [index - 1, index + 1, index - width, index + width]
                    .into_iter()
                    .filter(|neighbor| original[*neighbor])
                    .count();
                if neighbors >= 3 {
                    active[index] = true;
                }
            }
        }
    }
}

fn connected_components(active: &[bool], width: usize, height: usize) -> Vec<Component> {
    let mut visited = vec![false; active.len()];
    let mut result = Vec::new();
    for start in 0..active.len() {
        if !active[start] || visited[start] {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        visited[start] = true;
        let mut component = Component {
            min_x: start % width,
            max_x: start % width,
            min_y: start / width,
            max_y: start / width,
            cells: 0,
        };
        while let Some(index) = queue.pop_front() {
            let x = index % width;
            let y = index / width;
            component.min_x = component.min_x.min(x);
            component.max_x = component.max_x.max(x);
            component.min_y = component.min_y.min(y);
            component.max_y = component.max_y.max(y);
            component.cells += 1;
            let neighbors = [
                x.checked_sub(1).map(|x| y * width + x),
                (x + 1 < width).then_some(y * width + x + 1),
                y.checked_sub(1).map(|y| y * width + x),
                (y + 1 < height).then_some((y + 1) * width + x),
            ];
            for neighbor in neighbors.into_iter().flatten() {
                if active[neighbor] && !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        result.push(component);
    }
    result
}

fn target_cells(slot: RectF, count: usize) -> Vec<RectF> {
    match count {
        2 => vec![
            RectF {
                height: slot.height * 0.5,
                ..slot
            },
            RectF {
                y: slot.y + slot.height * 0.5,
                height: slot.height * 0.5,
                ..slot
            },
        ],
        3 => vec![
            RectF {
                x: slot.x + slot.width * 0.25,
                width: slot.width * 0.5,
                height: slot.height * 0.5,
                ..slot
            },
            RectF {
                y: slot.y + slot.height * 0.5,
                width: slot.width * 0.5,
                height: slot.height * 0.5,
                ..slot
            },
            RectF {
                x: slot.x + slot.width * 0.5,
                y: slot.y + slot.height * 0.5,
                width: slot.width * 0.5,
                height: slot.height * 0.5,
            },
        ],
        4 => (0..4)
            .map(|index| RectF {
                x: slot.x + (index % 2) as f32 * slot.width * 0.5,
                y: slot.y + (index / 2) as f32 * slot.height * 0.5,
                width: slot.width * 0.5,
                height: slot.height * 0.5,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn inset(rect: RectF, amount: f32) -> RectF {
    RectF {
        x: rect.x + amount,
        y: rect.y + amount,
        width: (rect.width - amount * 2.0).max(1.0),
        height: (rect.height - amount * 2.0).max(1.0),
    }
}

fn similar(left: &ParticipantDetection, right: &ParticipantDetection) -> bool {
    left.crops.len() == right.crops.len()
        && left.crops.iter().zip(&right.crops).all(|(left, right)| {
            (left.x - right.x).abs() < 0.06
                && (left.y - right.y).abs() < 0.06
                && (left.width - right.width).abs() < 0.08
                && (left.height - right.height).abs() < 0.08
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(rects: &[(u32, u32, u32, u32)]) -> CpuFrame {
        let (width, height) = (480u32, 270u32);
        let mut pixels = vec![0x30; width as usize * height as usize * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 0xFF;
        }
        for (index, &(x, y, w, h)) in rects.iter().enumerate() {
            let color = [0x90 + index as u8 * 20, 0x70, 0xC0, 0xFF];
            for py in y..y + h {
                for px in x..x + w {
                    let offset = (py as usize * width as usize + px as usize) * 4;
                    pixels[offset..offset + 4].copy_from_slice(&color);
                }
            }
        }
        CpuFrame {
            pixels,
            width,
            height,
            captured_qpc_100ns: 0,
            generation: 1,
        }
    }

    #[test]
    fn detects_two_three_and_four_separated_tiles() {
        let cases = [
            vec![(20, 20, 210, 230), (250, 20, 210, 230)],
            vec![
                (135, 12, 210, 115),
                (20, 143, 210, 115),
                (250, 143, 210, 115),
            ],
            vec![
                (20, 12, 210, 115),
                (250, 12, 210, 115),
                (20, 143, 210, 115),
                (250, 143, 210, 115),
            ],
        ];
        for (index, rects) in cases.iter().enumerate() {
            let detected = detect_participants(&synthetic(rects)).unwrap();
            assert_eq!(detected.crops.len(), index + 2);
            assert!(detected.confidence > 0.7);
        }
    }

    #[test]
    fn tracker_requires_stability_and_falls_back_after_misses() {
        let frame = synthetic(&[(20, 20, 210, 230), (250, 20, 210, 230)]);
        let blank = synthetic(&[]);
        let mut tracker = ParticipantTracker::default();
        assert!(tracker.update(&frame).is_none());
        assert!(tracker.update(&frame).is_none());
        assert_eq!(tracker.update(&frame).unwrap().crops.len(), 2);
        for _ in 0..MAX_MISSED_OBSERVATIONS - 1 {
            assert!(tracker.update(&blank).is_some());
        }
        assert!(tracker.update(&blank).is_none());
    }

    #[test]
    fn unsupported_or_ambiguous_counts_keep_the_whole_window_fallback() {
        assert!(detect_participants(&synthetic(&[(20, 20, 440, 230)])).is_none());
        assert!(detect_participants(&synthetic(&[
            (10, 10, 140, 110),
            (170, 10, 140, 110),
            (330, 10, 140, 110),
            (90, 145, 140, 110),
            (250, 145, 140, 110),
        ]))
        .is_none());
    }

    #[test]
    fn participant_placements_reflow_every_supported_count_inside_reaction_slot() {
        for count in 2..=4 {
            let detection = ParticipantDetection {
                crops: (0..count)
                    .map(|_| RectF {
                        x: 0.1,
                        y: 0.1,
                        width: 0.35,
                        height: 0.4,
                    })
                    .collect(),
                confidence: 0.9,
            };
            let placements = participant_placements(
                WatchPartyLayout::ReactionsRight,
                1920,
                1080,
                (1280, 720),
                &detection,
            );
            assert_eq!(placements.len(), count);
            assert!(placements
                .iter()
                .all(|placement| placement.destination.x >= 1382.0));
            assert!(placements.iter().all(|placement| {
                placement.source_uv.x >= 0.0
                    && placement.source_uv.y >= 0.0
                    && placement.source_uv.x + placement.source_uv.width <= 1.0
                    && placement.source_uv.y + placement.source_uv.height <= 1.0
            }));
        }
    }
}
