use super::apca_contrast::apca_contrast;
use super::*;
use gpui::{AbsoluteLength, Hsla, font};

#[test]
fn test_is_decorative_character() {
    // Box Drawing characters (U+2500 to U+257F)
    assert!(TerminalElement::is_decorative_character('─')); // U+2500
    assert!(TerminalElement::is_decorative_character('│')); // U+2502
    assert!(TerminalElement::is_decorative_character('┌')); // U+250C
    assert!(TerminalElement::is_decorative_character('┐')); // U+2510
    assert!(TerminalElement::is_decorative_character('└')); // U+2514
    assert!(TerminalElement::is_decorative_character('┘')); // U+2518
    assert!(TerminalElement::is_decorative_character('┼')); // U+253C

    // Block Elements (U+2580 to U+259F)
    assert!(TerminalElement::is_decorative_character('▀')); // U+2580
    assert!(TerminalElement::is_decorative_character('▄')); // U+2584
    assert!(TerminalElement::is_decorative_character('█')); // U+2588
    assert!(TerminalElement::is_decorative_character('░')); // U+2591
    assert!(TerminalElement::is_decorative_character('▒')); // U+2592
    assert!(TerminalElement::is_decorative_character('▓')); // U+2593

    // Geometric Shapes - block/box-like subset (U+25A0 to U+25D7)
    assert!(TerminalElement::is_decorative_character('■')); // U+25A0
    assert!(TerminalElement::is_decorative_character('□')); // U+25A1
    assert!(TerminalElement::is_decorative_character('▲')); // U+25B2
    assert!(TerminalElement::is_decorative_character('▼')); // U+25BC
    assert!(TerminalElement::is_decorative_character('◆')); // U+25C6
    assert!(TerminalElement::is_decorative_character('●')); // U+25CF

    // The specific character from the issue
    assert!(TerminalElement::is_decorative_character('◗')); // U+25D7
    assert!(TerminalElement::is_decorative_character('◘')); // U+25D8 (now included in Geometric Shapes)
    assert!(TerminalElement::is_decorative_character('◙')); // U+25D9 (now included in Geometric Shapes)

    // Powerline symbols (Private Use Area)
    assert!(TerminalElement::is_decorative_character('\u{E0B0}')); // Powerline right triangle
    assert!(TerminalElement::is_decorative_character('\u{E0B2}')); // Powerline left triangle
    assert!(TerminalElement::is_decorative_character('\u{E0B4}')); // Powerline right half circle (the actual issue!)
    assert!(TerminalElement::is_decorative_character('\u{E0B6}')); // Powerline left half circle
    assert!(TerminalElement::is_decorative_character('\u{E0CA}')); // Powerline mirrored ice waveform
    assert!(TerminalElement::is_decorative_character('\u{E0D7}')); // Powerline left triangle inverted

    // Characters that should NOT be considered decorative
    assert!(!TerminalElement::is_decorative_character('A')); // Regular letter
    assert!(!TerminalElement::is_decorative_character('$')); // Symbol
    assert!(!TerminalElement::is_decorative_character(' ')); // Space
    assert!(!TerminalElement::is_decorative_character('←')); // U+2190 (Arrow, not in our ranges)
    assert!(!TerminalElement::is_decorative_character('→')); // U+2192 (Arrow, not in our ranges)
    assert!(!TerminalElement::is_decorative_character('\u{F00C}')); // Font Awesome check (icon, needs contrast)
    assert!(!TerminalElement::is_decorative_character('\u{E711}')); // Devicons (icon, needs contrast)
    assert!(!TerminalElement::is_decorative_character('\u{EA71}')); // Codicons folder (icon, needs contrast)
    assert!(!TerminalElement::is_decorative_character('\u{F401}')); // Octicons (icon, needs contrast)
    assert!(!TerminalElement::is_decorative_character('\u{1F600}')); // Emoji (not in our ranges)
}

#[test]
fn test_decorative_character_boundary_cases() {
    // Test exact boundaries of our ranges
    // Box Drawing range boundaries
    assert!(TerminalElement::is_decorative_character('\u{2500}')); // First char
    assert!(TerminalElement::is_decorative_character('\u{257F}')); // Last char
    assert!(!TerminalElement::is_decorative_character('\u{24FF}')); // Just before

    // Block Elements range boundaries
    assert!(TerminalElement::is_decorative_character('\u{2580}')); // First char
    assert!(TerminalElement::is_decorative_character('\u{259F}')); // Last char

    // Geometric Shapes subset boundaries
    assert!(TerminalElement::is_decorative_character('\u{25A0}')); // First char
    assert!(TerminalElement::is_decorative_character('\u{25FF}')); // Last char
    assert!(!TerminalElement::is_decorative_character('\u{2600}')); // Just after

    // Sextant range boundaries
    assert!(TerminalElement::is_decorative_character('\u{1FB00}')); // First char
    assert!(TerminalElement::is_decorative_character('\u{1FB3B}')); // Last char
    assert!(!TerminalElement::is_decorative_character('\u{1FAFF}')); // Just before
    assert!(!TerminalElement::is_decorative_character('\u{1FB3C}')); // Just after
}

#[test]
fn test_sextant_char_to_filled_bits() {
    // U+1FB00 BLOCK SEXTANT-1: only the top-left subcell.
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB00}'),
        Some(0b00_0001)
    );
    // U+1FB13 BLOCK SEXTANT-35 and U+1FB14 BLOCK SEXTANT-235 straddle the
    // gap left by `▌` (0b01_0101).
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB13}'),
        Some(0b01_0100)
    );
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB14}'),
        Some(0b01_0110)
    );
    // U+1FB3B BLOCK SEXTANT-12356: everything except the bottom-left subcell.
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB3B}'),
        Some(0b11_1110)
    );
    assert_eq!(TerminalElement::sextant_char_to_filled_bits('█'), None);
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB3C}'),
        None
    );
}

#[test]
fn test_block_element_rects_merge_across_adjacent_full_blocks() {
    let color = Hsla::default();
    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '█',
        color,
        &mut regions,
    ));
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 1),
        '█',
        color,
        &mut regions,
    ));

    assert_eq!(
        regions.len(),
        1,
        "adjacent full blocks should be merged eagerly by push_block_element_region"
    );

    let rects = TerminalElement::block_element_regions_to_rects(regions);

    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].point.line(), 0);
    assert_eq!(rects[0].point.column(), 0);
    assert_eq!(rects[0].num_of_columns, 16);
    assert_eq!(rects[0].num_of_lines, 24);
    assert_eq!(rects[0].line(), 0);
}

#[test]
fn test_block_element_rects_cover_half_blocks_and_sextants() {
    let color = Hsla::default();
    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '▄',
        color,
        &mut regions,
    ));
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(1, 0),
        '\u{1FB00}',
        color,
        &mut regions,
    ));

    let rects = TerminalElement::block_element_regions_to_rects(regions);

    assert!(rects.iter().any(|rect| {
        rect.point.line() == 12
            && rect.point.column() == 0
            && rect.num_of_columns == 8
            && rect.num_of_lines == 12
    }));
    assert!(rects.iter().any(|rect| {
        rect.point.line() == 24
            && rect.point.column() == 0
            && rect.num_of_columns == 4
            && rect.num_of_lines == 8
    }));
}

#[test]
fn test_block_element_rects_cover_eighth_blocks() {
    let color = Hsla::default();

    for (ch, expected_column, expected_line, expected_columns, expected_lines) in [
        ('▁', 0, 21, 8, 3),
        ('▇', 0, 3, 8, 21),
        ('▉', 0, 0, 7, 24),
        ('▏', 0, 0, 1, 24),
        ('▔', 0, 0, 8, 3),
        ('▕', 7, 0, 1, 24),
    ] {
        let mut regions = Vec::new();
        assert!(TerminalElement::collect_block_element_regions(
            LayoutPoint::new(0, 0),
            ch,
            color,
            &mut regions,
        ));
        let rects = TerminalElement::block_element_regions_to_rects(regions);

        assert_eq!(rects.len(), 1, "unexpected rect count for {ch}");
        assert_eq!(rects[0].point.column(), expected_column, "column for {ch}");
        assert_eq!(rects[0].point.line(), expected_line, "line for {ch}");
        assert_eq!(
            rects[0].num_of_columns, expected_columns,
            "columns for {ch}"
        );
        assert_eq!(rects[0].num_of_lines, expected_lines, "lines for {ch}");
    }
}

#[test]
fn test_block_element_rects_cover_quadrants() {
    let color = Hsla::default();
    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '▚',
        color,
        &mut regions,
    ));

    let rects = TerminalElement::block_element_regions_to_rects(regions);

    assert_eq!(rects.len(), 2);
    assert!(rects.iter().any(|rect| {
        rect.point.line() == 0
            && rect.point.column() == 0
            && rect.num_of_columns == 4
            && rect.num_of_lines == 12
    }));
    assert!(rects.iter().any(|rect| {
        rect.point.line() == 12
            && rect.point.column() == 4
            && rect.num_of_columns == 4
            && rect.num_of_lines == 12
    }));
}

#[test]
fn test_block_element_rects_cover_shades() {
    let color = gpui::red();
    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '▒',
        color,
        &mut regions,
    ));

    let rects = TerminalElement::block_element_regions_to_rects(regions);

    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].num_of_columns, 8);
    assert_eq!(rects[0].num_of_lines, 24);
    assert_eq!(rects[0].color, color.opacity(0.5));
}

#[test]
fn test_block_element_chars_fully_handled_within_cell() {
    let color = Hsla::default();

    for codepoint in (0x2580..=0x259F).chain(0x1FB00..=0x1FB3B) {
        let ch = char::from_u32(codepoint).expect("valid block element codepoint");
        let mut regions = Vec::new();
        assert!(
            TerminalElement::collect_block_element_regions(
                LayoutPoint::new(0, 0),
                ch,
                color,
                &mut regions,
            ),
            "U+{codepoint:04X} {ch} should be custom-painted"
        );
        assert!(
            !regions.is_empty(),
            "U+{codepoint:04X} {ch} should fill at least one subcell"
        );

        let mut filled = [[false; BLOCK_SUBCELL_COLUMNS as usize]; BLOCK_SUBCELL_LINES as usize];
        for region in &regions {
            assert!(
                region.start_line <= region.end_line
                    && region.start_col <= region.end_col
                    && (0..BLOCK_SUBCELL_LINES).contains(&region.start_line)
                    && (0..BLOCK_SUBCELL_LINES).contains(&region.end_line)
                    && (0..BLOCK_SUBCELL_COLUMNS).contains(&region.start_col)
                    && (0..BLOCK_SUBCELL_COLUMNS).contains(&region.end_col),
                "U+{codepoint:04X} {ch} paints outside its cell: {region:?}"
            );
            for line in region.start_line..=region.end_line {
                for column in region.start_col..=region.end_col {
                    assert!(
                        !filled[line as usize][column as usize],
                        "U+{codepoint:04X} {ch} paints subcell ({line}, {column}) twice"
                    );
                    filled[line as usize][column as usize] = true;
                }
            }
        }
    }
}

#[test]
fn test_decorative_characters_bypass_contrast_adjustment() {
    // Decorative characters should not be affected by contrast adjustment

    // The specific character from issue #34234
    let problematic_char = '◗'; // U+25D7
    assert!(
        TerminalElement::is_decorative_character(problematic_char),
        "Character ◗ (U+25D7) should be recognized as decorative"
    );

    // Verify some other commonly used decorative characters
    assert!(TerminalElement::is_decorative_character('│')); // Vertical line
    assert!(TerminalElement::is_decorative_character('─')); // Horizontal line
    assert!(TerminalElement::is_decorative_character('█')); // Full block
    assert!(TerminalElement::is_decorative_character('▓')); // Dark shade
    assert!(TerminalElement::is_decorative_character('■')); // Black square
    assert!(TerminalElement::is_decorative_character('●')); // Black circle

    // Verify normal text characters are NOT decorative
    assert!(!TerminalElement::is_decorative_character('A'));
    assert!(!TerminalElement::is_decorative_character('1'));
    assert!(!TerminalElement::is_decorative_character('$'));
    assert!(!TerminalElement::is_decorative_character(' '));
}

#[test]
fn test_is_app_chosen_exact_color() {
    use terminal::{Color, NamedColor, Rgb};

    // Indices 0..=15 are theme-overridable ANSI colors; contrast adjustment must still apply.
    assert!(!TerminalElement::is_app_chosen_exact_color(
        &Color::Indexed(0)
    ));
    assert!(!TerminalElement::is_app_chosen_exact_color(
        &Color::Indexed(15)
    ));

    // Boundary: index 16 is the first entry of the 6x6x6 cube — application-chosen.
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Indexed(
        16
    )));
    // Interior of the cube.
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Indexed(
        17
    )));
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Indexed(
        231
    )));
    // Grayscale ramp boundaries.
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Indexed(
        232
    )));
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Indexed(
        255
    )));

    // 24-bit true color is always application-chosen.
    assert!(TerminalElement::is_app_chosen_exact_color(&Color::Spec(
        Rgb {
            r: 10,
            g: 20,
            b: 30
        }
    )));

    // Named colors are theme-defined and must go through contrast adjustment.
    assert!(!TerminalElement::is_app_chosen_exact_color(&Color::Named(
        NamedColor::Red
    )));
    assert!(!TerminalElement::is_app_chosen_exact_color(&Color::Named(
        NamedColor::Foreground
    )));
}

#[test]
fn test_contrast_adjustment_logic() {
    // Test the core contrast adjustment logic without needing full app context

    // Test case 1: Light colors (poor contrast)
    let white_fg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 1.0,
    };
    let light_gray_bg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.95,
        a: 1.0,
    };

    // Should have poor contrast
    let actual_contrast = apca_contrast(white_fg, light_gray_bg).abs();
    assert!(
        actual_contrast < 30.0,
        "White on light gray should have poor APCA contrast: {}",
        actual_contrast
    );

    // After adjustment with minimum APCA contrast of 45, should be darker
    let adjusted = ensure_minimum_contrast(white_fg, light_gray_bg, 45.0);
    assert!(
        adjusted.l < white_fg.l,
        "Adjusted color should be darker than original"
    );
    let adjusted_contrast = apca_contrast(adjusted, light_gray_bg).abs();
    assert!(adjusted_contrast >= 45.0, "Should meet minimum contrast");

    // Test case 2: Dark colors (poor contrast)
    let black_fg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 1.0,
    };
    let dark_gray_bg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.05,
        a: 1.0,
    };

    // Should have poor contrast
    let actual_contrast = apca_contrast(black_fg, dark_gray_bg).abs();
    assert!(
        actual_contrast < 30.0,
        "Black on dark gray should have poor APCA contrast: {}",
        actual_contrast
    );

    // After adjustment with minimum APCA contrast of 45, should be lighter
    let adjusted = ensure_minimum_contrast(black_fg, dark_gray_bg, 45.0);
    assert!(
        adjusted.l > black_fg.l,
        "Adjusted color should be lighter than original"
    );
    let adjusted_contrast = apca_contrast(adjusted, dark_gray_bg).abs();
    assert!(adjusted_contrast >= 45.0, "Should meet minimum contrast");

    // Test case 3: Already good contrast
    let good_contrast = ensure_minimum_contrast(black_fg, white_fg, 45.0);
    assert_eq!(
        good_contrast, black_fg,
        "Good contrast should not be adjusted"
    );
}

#[test]
fn test_true_color_red_blue_not_washed_out_on_dark_bg() {
    // Red and blue have inherently low perceptual luminance in APCA.
    // Pure #ff0000 only achieves Lc ~35 against #1e1e1e — below the
    // default Lc 45 threshold. ensure_minimum_contrast would lighten
    // them, washing out the color. This is why cell_style skips the
    // adjustment for Color::Spec (24-bit true color).
    let dark_bg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.05,
        a: 1.0,
    };

    for (name, r, g, b) in [
        ("red", 225, 80, 80),
        ("blue", 80, 80, 225),
        ("pure red", 255, 0, 0),
    ] {
        let color = terminal::rgba_color(r, g, b);
        let contrast = apca_contrast(color, dark_bg).abs();
        assert!(
            contrast < 45.0,
            "{name} should have APCA < 45 on dark bg, got {contrast}",
        );

        let adjusted = ensure_minimum_contrast(color, dark_bg, 45.0);
        assert!(
            adjusted.l > color.l,
            "{name} would be lightened by contrast adjustment (l: {} -> {})",
            color.l,
            adjusted.l,
        );
    }
}

#[test]
fn test_white_on_white_contrast_issue() {
    // This test reproduces the exact issue from the bug report
    // where white ANSI text on white background should be adjusted

    // Simulate One Light theme colors
    let white_fg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.98, // #fafafaff is approximately 98% lightness
        a: 1.0,
    };
    let white_bg = gpui::Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.98, // Same as foreground - this is the problem!
        a: 1.0,
    };

    // With minimum contrast of 0.0, no adjustment should happen
    let no_adjust = ensure_minimum_contrast(white_fg, white_bg, 0.0);
    assert_eq!(no_adjust, white_fg, "No adjustment with min_contrast 0.0");

    // With minimum APCA contrast of 15, it should adjust to a darker color
    let adjusted = ensure_minimum_contrast(white_fg, white_bg, 15.0);
    assert!(
        adjusted.l < white_fg.l,
        "White on white should become darker, got l={}",
        adjusted.l
    );

    // Verify the contrast is now acceptable
    let new_contrast = apca_contrast(adjusted, white_bg).abs();
    assert!(
        new_contrast >= 15.0,
        "Adjusted APCA contrast {} should be >= 15.0",
        new_contrast
    );
}

#[test]
fn test_batched_text_run_can_append() {
    let style1 = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::red(),
        ..Default::default()
    };

    let style2 = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::red(),
        ..Default::default()
    };

    let style3 = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::blue(), // Different color
        ..Default::default()
    };

    let font_size = AbsoluteLength::Pixels(px(12.0));
    let batch = BatchedTextRun::new_from_char(LayoutPoint::new(0, 0), 'a', style1, font_size);

    // Should be able to append same style
    assert!(batch.can_append(&style2));

    // Should not be able to append different style
    assert!(!batch.can_append(&style3));
}

#[test]
fn test_batched_text_run_append() {
    let style = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::red(),
        ..Default::default()
    };

    let font_size = AbsoluteLength::Pixels(px(12.0));
    let mut batch = BatchedTextRun::new_from_char(LayoutPoint::new(0, 0), 'a', style, font_size);

    assert_eq!(batch.text, "a");
    assert_eq!(batch.cell_count, 1);
    assert_eq!(batch.style.len, 1);

    batch.append_char('b');

    assert_eq!(batch.text, "ab");
    assert_eq!(batch.cell_count, 2);
    assert_eq!(batch.style.len, 2);

    batch.append_char('c');

    assert_eq!(batch.text, "abc");
    assert_eq!(batch.cell_count, 3);
    assert_eq!(batch.style.len, 3);
}

#[test]
fn test_batched_text_run_append_char() {
    let style = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::red(),
        ..Default::default()
    };

    let font_size = AbsoluteLength::Pixels(px(12.0));
    let mut batch = BatchedTextRun::new_from_char(LayoutPoint::new(0, 0), 'x', style, font_size);

    assert_eq!(batch.text, "x");
    assert_eq!(batch.cell_count, 1);
    assert_eq!(batch.style.len, 1);

    batch.append_char('y');

    assert_eq!(batch.text, "xy");
    assert_eq!(batch.cell_count, 2);
    assert_eq!(batch.style.len, 2);

    // Test with multi-byte character
    batch.append_char('😀');

    assert_eq!(batch.text, "xy😀");
    assert_eq!(batch.cell_count, 3);
    assert_eq!(batch.style.len, 6); // 1 + 1 + 4 bytes for emoji
}

#[test]
fn test_batched_text_run_append_zero_width_char() {
    let style = TextRun {
        len: 1,
        font: font("Helvetica"),
        color: Hsla::red(),
        ..Default::default()
    };

    let font_size = AbsoluteLength::Pixels(px(12.0));
    let mut batch = BatchedTextRun::new_from_char(LayoutPoint::new(0, 0), 'x', style, font_size);

    let combining = '\u{0301}';
    batch.append_zero_width_chars(&[combining]);

    assert_eq!(batch.text, format!("x{}", combining));
    assert_eq!(batch.cell_count, 1);
    assert_eq!(batch.style.len, 1 + combining.len_utf8());
}

#[test]
fn test_background_region_can_merge() {
    let color1 = Hsla::red();
    let color2 = Hsla::blue();

    // Test horizontal merging
    let mut region1 = BackgroundRegion::new(0, 0, color1);
    region1.end_col = 5;
    let region2 = BackgroundRegion::new(0, 6, color1);
    assert!(region1.can_merge_with(&region2));

    // Test vertical merging with same column span
    let mut region3 = BackgroundRegion::new(0, 0, color1);
    region3.end_col = 5;
    let mut region4 = BackgroundRegion::new(1, 0, color1);
    region4.end_col = 5;
    assert!(region3.can_merge_with(&region4));

    // Test cannot merge different colors
    let region5 = BackgroundRegion::new(0, 0, color1);
    let region6 = BackgroundRegion::new(0, 1, color2);
    assert!(!region5.can_merge_with(&region6));

    // Test cannot merge non-adjacent regions
    let region7 = BackgroundRegion::new(0, 0, color1);
    let region8 = BackgroundRegion::new(0, 2, color1);
    assert!(!region7.can_merge_with(&region8));

    // Test cannot merge vertical regions with different column spans
    let mut region9 = BackgroundRegion::new(0, 0, color1);
    region9.end_col = 5;
    let mut region10 = BackgroundRegion::new(1, 0, color1);
    region10.end_col = 6;
    assert!(!region9.can_merge_with(&region10));
}

#[test]
fn test_background_region_merge() {
    let color = Hsla::red();

    // Test horizontal merge
    let mut region1 = BackgroundRegion::new(0, 0, color);
    region1.end_col = 5;
    let mut region2 = BackgroundRegion::new(0, 6, color);
    region2.end_col = 10;
    region1.merge_with(&region2);
    assert_eq!(region1.start_col, 0);
    assert_eq!(region1.end_col, 10);
    assert_eq!(region1.start_line, 0);
    assert_eq!(region1.end_line, 0);

    // Test vertical merge
    let mut region3 = BackgroundRegion::new(0, 0, color);
    region3.end_col = 5;
    let mut region4 = BackgroundRegion::new(1, 0, color);
    region4.end_col = 5;
    region3.merge_with(&region4);
    assert_eq!(region3.start_col, 0);
    assert_eq!(region3.end_col, 5);
    assert_eq!(region3.start_line, 0);
    assert_eq!(region3.end_line, 1);
}

#[test]
fn test_merge_background_regions() {
    let color = Hsla::red();

    // Test merging multiple adjacent regions
    let regions = vec![
        BackgroundRegion::new(0, 0, color),
        BackgroundRegion::new(0, 1, color),
        BackgroundRegion::new(0, 2, color),
        BackgroundRegion::new(1, 0, color),
        BackgroundRegion::new(1, 1, color),
        BackgroundRegion::new(1, 2, color),
    ];

    let merged = merge_background_regions(regions);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].start_line, 0);
    assert_eq!(merged[0].end_line, 1);
    assert_eq!(merged[0].start_col, 0);
    assert_eq!(merged[0].end_col, 2);

    // Test with non-mergeable regions
    let color2 = Hsla::blue();
    let regions2 = vec![
        BackgroundRegion::new(0, 0, color),
        BackgroundRegion::new(0, 2, color),  // Gap at column 1
        BackgroundRegion::new(1, 0, color2), // Different color
    ];

    let merged2 = merge_background_regions(regions2);
    assert_eq!(merged2.len(), 3);
}

#[test]
fn test_screen_position_filtering_with_positive_lines() {
    // Test the unified screen-position-based filtering approach.
    // This works for both Scrollable and Inline modes because we filter
    // by enumerated line group index, not by cell.point.line values.
    use itertools::Itertools;
    use terminal::{Cell, IndexedCell, Point};

    // Create mock cells for lines 0-23 (typical terminal with 24 visible lines)
    let mut cells = Vec::new();
    for line in 0..24i32 {
        for col in 0..3i32 {
            cells.push(IndexedCell {
                point: Point::new(line, col as usize),
                cell: Cell::default(),
            });
        }
    }

    // Scenario: Terminal partially scrolled above viewport
    // First 5 lines (0-4) are clipped, lines 5-15 should be visible
    let rows_above_viewport = 5usize;
    let visible_row_count = 11usize;

    // Apply the same filtering logic as in the render code
    let filtered: Vec<_> = cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .skip(rows_above_viewport)
        .take(visible_row_count)
        .flat_map(|(_, line_cells)| line_cells)
        .collect();

    // Should have lines 5-15 (11 lines * 3 cells each = 33 cells)
    assert_eq!(filtered.len(), 11 * 3, "Should have 33 cells for 11 lines");

    // First filtered cell should be line 5
    assert_eq!(
        filtered.first().unwrap().point.line,
        5,
        "First cell should be on line 5"
    );

    // Last filtered cell should be line 15
    assert_eq!(
        filtered.last().unwrap().point.line,
        15,
        "Last cell should be on line 15"
    );
}

#[test]
fn test_screen_position_filtering_with_negative_lines() {
    // This is the key test! In Scrollable mode, cells have NEGATIVE line numbers
    // for scrollback history. The screen-position filtering approach works because
    // we filter by enumerated line group index, not by cell.point.line values.
    use itertools::Itertools;
    use terminal::{Cell, IndexedCell, Point};

    // Simulate cells from a scrolled terminal with scrollback
    // These have negative line numbers representing scrollback history
    let mut scrollback_cells = Vec::new();
    for line in -588i32..=-578i32 {
        for col in 0..80i32 {
            scrollback_cells.push(IndexedCell {
                point: Point::new(line, col as usize),
                cell: Cell::default(),
            });
        }
    }

    // Scenario: First 3 screen rows clipped, show next 5 rows
    let rows_above_viewport = 3usize;
    let visible_row_count = 5usize;

    // Apply the same filtering logic as in the render code
    let filtered: Vec<_> = scrollback_cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .skip(rows_above_viewport)
        .take(visible_row_count)
        .flat_map(|(_, line_cells)| line_cells)
        .collect();

    // Should have 5 lines * 80 cells = 400 cells
    assert_eq!(filtered.len(), 5 * 80, "Should have 400 cells for 5 lines");

    // First filtered cell should be line -585 (skipped 3 lines from -588)
    assert_eq!(
        filtered.first().unwrap().point.line,
        -585,
        "First cell should be on line -585"
    );

    // Last filtered cell should be line -581 (5 lines: -585, -584, -583, -582, -581)
    assert_eq!(
        filtered.last().unwrap().point.line,
        -581,
        "Last cell should be on line -581"
    );
}

#[test]
fn test_screen_position_filtering_skip_all() {
    // Test what happens when we skip more rows than exist
    use itertools::Itertools;
    use terminal::{Cell, IndexedCell, Point};

    let mut cells = Vec::new();
    for line in 0..10i32 {
        cells.push(IndexedCell {
            point: Point::new(line, 0),
            cell: Cell::default(),
        });
    }

    // Skip more rows than exist
    let rows_above_viewport = 100usize;
    let visible_row_count = 5usize;

    let filtered: Vec<_> = cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .skip(rows_above_viewport)
        .take(visible_row_count)
        .flat_map(|(_, line_cells)| line_cells)
        .collect();

    assert_eq!(
        filtered.len(),
        0,
        "Should have no cells when all are skipped"
    );
}

#[test]
fn test_layout_grid_positioning_math() {
    // Test the math that layout_grid uses for positioning.
    // When we skip N rows, we pass N as start_line_offset to layout_grid,
    // which positions the first visible line at screen row N.

    // Scenario: Terminal at y=-100px, line_height=20px
    // First 5 screen rows are above viewport (clipped)
    // So we skip 5 rows and pass offset=5 to layout_grid

    let terminal_origin_y = -100.0f32;
    let line_height = 20.0f32;
    let rows_skipped = 5;

    // The first visible line (at offset 5) renders at:
    // y = terminal_origin + offset * line_height = -100 + 5*20 = 0
    let first_visible_y = terminal_origin_y + rows_skipped as f32 * line_height;
    assert_eq!(
        first_visible_y, 0.0,
        "First visible line should be at viewport top (y=0)"
    );

    // The 6th visible line (at offset 10) renders at:
    let sixth_visible_y = terminal_origin_y + (rows_skipped + 5) as f32 * line_height;
    assert_eq!(
        sixth_visible_y, 100.0,
        "6th visible line should be at y=100"
    );
}

#[test]
fn test_unified_filtering_works_for_both_modes() {
    // This test proves that the unified screen-position filtering approach
    // works for BOTH positive line numbers (Inline mode) and negative line
    // numbers (Scrollable mode with scrollback).
    //
    // The key insight: we filter by enumerated line group index (screen position),
    // not by cell.point.line values. This makes the filtering agnostic to the
    // actual line numbers in the cells.
    use itertools::Itertools;
    use terminal::Point;
    use terminal::{Cell, IndexedCell};

    // Test with positive line numbers (Inline mode style)
    let positive_cells: Vec<_> = (0..10i32)
        .flat_map(|line| {
            (0..3i32).map(move |col| IndexedCell {
                point: Point::new(line, col as usize),
                cell: Cell::default(),
            })
        })
        .collect();

    // Test with negative line numbers (Scrollable mode with scrollback)
    let negative_cells: Vec<_> = (-10i32..0i32)
        .flat_map(|line| {
            (0..3i32).map(move |col| IndexedCell {
                point: Point::new(line, col as usize),
                cell: Cell::default(),
            })
        })
        .collect();

    let rows_to_skip = 3usize;
    let rows_to_take = 4usize;

    // Filter positive cells
    let positive_filtered: Vec<_> = positive_cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .skip(rows_to_skip)
        .take(rows_to_take)
        .flat_map(|(_, cells)| cells)
        .collect();

    // Filter negative cells
    let negative_filtered: Vec<_> = negative_cells
        .iter()
        .chunk_by(|c| c.point.line)
        .into_iter()
        .skip(rows_to_skip)
        .take(rows_to_take)
        .flat_map(|(_, cells)| cells)
        .collect();

    // Both should have same count: 4 lines * 3 cells = 12
    assert_eq!(positive_filtered.len(), 12);
    assert_eq!(negative_filtered.len(), 12);

    // Positive: lines 3, 4, 5, 6
    assert_eq!(positive_filtered.first().unwrap().point.line, 3);
    assert_eq!(positive_filtered.last().unwrap().point.line, 6);

    // Negative: lines -7, -6, -5, -4
    assert_eq!(negative_filtered.first().unwrap().point.line, -7);
    assert_eq!(negative_filtered.last().unwrap().point.line, -4);
}
