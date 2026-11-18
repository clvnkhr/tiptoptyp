#![allow(dead_code, unused_imports)]
use eframe::egui::{self, *};
use egui::text::{CCursor, CharIndex};
use egui::epaint::text::cursor::LayoutCursor;
use std::ops::Range;
mod editor_layout {use super::*;
#[derive(Clone, Copy)]
struct CursorRowEnd {
    chars: usize,
    newline: bool,
}

pub(super) struct CursorIndex {
    rows: Box<[CursorRowEnd]>,
}

impl CursorIndex {
    pub(super) fn new(galley: &Galley) -> Self {
        let mut chars = 0;
        let rows = galley
            .rows
            .iter()
            .map(|row| {
                let end = chars + row.char_count_excluding_newline().0;
                chars += row.char_count_including_newline().0;
                CursorRowEnd {
                    chars: end,
                    newline: row.ends_with_newline,
                }
            })
            .collect();
        Self { rows }
    }

    pub(super) fn layout(&self, galley: &Galley, cursor: CCursor) -> LayoutCursor {
        debug_assert_eq!(self.rows.len(), galley.rows.len());
        let index = cursor.index.0;
        let row = self.rows.partition_point(|end| {
            end.chars < index || (end.chars == index && cursor.prefer_next_row && !end.newline)
        });
        if let Some(current) = self.rows.get(row) {
            let start = row.checked_sub(1).map_or(0, |previous| {
                let previous = self.rows[previous];
                previous.chars + usize::from(previous.newline)
            });
            debug_assert!(index <= current.chars);
            LayoutCursor {
                row,
                column: CharIndex(index.saturating_sub(start)),
            }
        } else if let Some(last) = galley.rows.last() {
            LayoutCursor {
                row: galley.rows.len() - 1,
                column: last.char_count_including_newline(),
            }
        } else {
            LayoutCursor::default()
        }
    }

    pub(super) fn rect(&self, galley: &Galley, cursor: CCursor) -> Rect {
        galley.pos_from_layout_cursor(&self.layout(galley, cursor))
    }
}

}
mod theme {pub struct Palette {pub accent:Color32} use super::*; pub fn palette(_: &Context)->Palette {Palette {accent:Color32::from_rgb(180,80,210)}}}
mod production {use super::*;
pub(super) fn legacy_delimiter_rects(galley: &egui::Galley, characters: Range<usize>) -> Vec<Rect> {
    let mut rectangles: Vec<Rect> = Vec::new();
    for character in characters {
        let mut cursor = CCursor::new(character);
        cursor.prefer_next_row = true;
        let position = galley.layout_from_cursor(cursor);
        let Some(row) = galley.rows.get(position.row) else {
            continue;
        };
        if row.size.y == 0.0 || position.column.0 >= row.glyphs.len() {
            continue;
        }
        let start = row.pos.x + row.x_offset(position.column);
        let end = row.pos.x + row.x_offset(egui::text::CharIndex(position.column.0 + 1));
        let rect = Rect::from_min_max(
            Pos2::new(start, row.min_y()),
            Pos2::new(end.max(start + 1.0), row.max_y()),
        );
        if let Some(previous) = rectangles.last_mut()
            && previous.top() == rect.top()
        {
            *previous = previous.union(rect);
        } else {
            rectangles.push(rect);
        }
    }
    rectangles
}

pub(super) fn delimiter_rects(
    galley: &egui::Galley,
    index: &editor_layout::CursorIndex,
    characters: Range<usize>,
) -> Vec<Rect> {
    let mut rectangles: Vec<Rect> = Vec::new();
    for character in characters {
        let mut cursor = CCursor::new(character);
        cursor.prefer_next_row = true;
        let position = index.layout(galley, cursor);
        let Some(row) = galley.rows.get(position.row) else {
            continue;
        };
        if row.size.y == 0.0 || position.column.0 >= row.glyphs.len() {
            continue;
        }
        let start = row.pos.x + row.x_offset(position.column);
        let end = row.pos.x + row.x_offset(egui::text::CharIndex(position.column.0 + 1));
        let rect = Rect::from_min_max(
            Pos2::new(start, row.min_y()),
            Pos2::new(end.max(start + 1.0), row.max_y()),
        );
        if let Some(previous) = rectangles.last_mut()
            && previous.top() == rect.top()
        {
            *previous = previous.union(rect);
        } else {
            rectangles.push(rect);
        }
    }
    rectangles
}

pub fn paint(ui: &mut Ui) {
 struct Output {response: Response, galley: std::sync::Arc<Galley>, galley_pos:Pos2}
 struct Rows {cursor:editor_layout::CursorIndex}
 let response=ui.allocate_exact_size(vec2(300.0,90.0),Sense::hover()).1;
 let galley=ui.painter().layout_no_wrap("(inside)".into(),FontId::monospace(16.0),Color32::WHITE);
 let row_index=Rows {cursor:editor_layout::CursorIndex::new(&galley)};
 let output=Output {galley_pos:response.rect.min,response,galley};
 let pair=[0..1,7..8];
 let delimiter_fill_slot=ui.painter().add(Shape::Noop);
let accent = theme::palette(ui.ctx()).accent;
                let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 48);
                let mut backgrounds = Vec::new();
                let painter = ui
                    .painter()
                    .with_clip_rect(output.response.rect.intersect(ui.clip_rect()));
                for endpoint in pair {
                    for rect in delimiter_rects(&output.galley, &row_index.cursor, endpoint) {
                        let rect = rect.translate(output.galley_pos.to_vec2());
                        backgrounds.push(egui::Shape::rect_filled(rect, 2, fill));
                    }
                }
                painter.set(delimiter_fill_slot, egui::Shape::Vec(backgrounds));
}
}
#[test]
fn match_has_background_without_outline() {
    fn visit(shape:&egui::Shape, fills:&mut usize, outlines:&mut usize) {
        match shape {
            egui::Shape::Vec(children)=>for child in children {visit(child,fills,outlines)},
            egui::Shape::Rect(rect)=>{
                if rect.fill.a()==48 {*fills+=1;}
                if !rect.stroke.is_empty() {*outlines+=1;}
            },
            _=>{}
        }
    }
    let ctx=egui::Context::default();
    let mut output=ctx.run_ui(Default::default(),production::paint);
    let (mut fills,mut outlines)=(0,0);
    for shape in &output.shapes {visit(&shape.shape,&mut fills,&mut outlines);}
    output.textures_delta.clear();
    output.drop_without_applying_deltas();
    assert_eq!(fills,2);
    assert_eq!(outlines,0,"matching delimiters must use backgrounds without outlines");
}

#[test]
fn cached_endpoint_coordinates_match_and_stay_bounded() {
 let ctx=Context::default();
 ctx.run_ui(Default::default(),|ui|{
  let source=format!("\\begin{{align}}\n{}\\end{{align}}", "x + y\n".repeat(20_000));
  let galley=ui.painter().layout_no_wrap(source.clone(),FontId::monospace(16.0),Color32::WHITE);
  let index=editor_layout::CursorIndex::new(&galley);
  let pair=[0..13,source.chars().count()-11..source.chars().count()];
  for range in &pair {assert_eq!(production::legacy_delimiter_rects(&galley,range.clone()),production::delimiter_rects(&galley,&index,range.clone()));}
  for indexed in [false,true] {
   for _ in 0..5 {for range in &pair {std::hint::black_box(if indexed {production::delimiter_rects(&galley,&index,range.clone())} else {production::legacy_delimiter_rects(&galley,range.clone())});}}
   for batch in 0..5 {
    let start=std::time::Instant::now();
    for _ in 0..100 {for range in &pair {std::hint::black_box(if indexed {production::delimiter_rects(&galley,&index,range.clone())} else {production::legacy_delimiter_rects(&galley,range.clone())});}}
    println!("endpoint_coordinates,indexed={indexed},batch={batch},queries=100,rows={},source_bytes={},ns={}",galley.rows.len(),source.len(),start.elapsed().as_nanos());
   }
  }
 }).drop_without_applying_deltas();
}
