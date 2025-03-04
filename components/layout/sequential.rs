/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Implements sequential traversals over the DOM and flow trees.

use app_units::Au;
use euclid::default::{Point2D, Rect, Size2D, Vector2D};
use servo_config::opts;
use style::servo::restyle_damage::ServoRestyleDamage;
use webrender_api::units::LayoutPoint;
use webrender_api::{ColorF, PropertyBinding, RectangleDisplayItem};

use crate::context::LayoutContext;
use crate::display_list::conversions::ToLayout;
use crate::display_list::items::{self, CommonDisplayItem, DisplayItem, DisplayListSection};
use crate::display_list::{DisplayListBuildState, StackingContextCollectionState};
use crate::floats::SpeculatedFloatPlacement;
use crate::flow::{Flow, FlowFlags, GetBaseFlow, ImmutableFlowUtils};
use crate::fragment::{CoordinateSystem, FragmentBorderBoxIterator};
use crate::generated_content::ResolveGeneratedContent;
use crate::incremental::RelayoutMode;
use crate::traversal::{
    AssignBSizes, AssignISizes, BubbleISizes, BuildDisplayList, InorderFlowTraversal,
    PostorderFlowTraversal, PreorderFlowTraversal,
};

pub fn resolve_generated_content(root: &mut dyn Flow, layout_context: &LayoutContext) {
    ResolveGeneratedContent::new(&layout_context).traverse(root, 0);
}

/// Run the main layout passes sequentially.
pub fn reflow(root: &mut dyn Flow, layout_context: &LayoutContext, relayout_mode: RelayoutMode) {
    fn doit(
        flow: &mut dyn Flow,
        assign_inline_sizes: AssignISizes,
        assign_block_sizes: AssignBSizes,
        relayout_mode: RelayoutMode,
    ) {
        // Force reflow children during this traversal. This is needed when we failed
        // the float speculation of a block formatting context and need to fix it.
        if relayout_mode == RelayoutMode::Force {
            flow.mut_base()
                .restyle_damage
                .insert(ServoRestyleDamage::REFLOW_OUT_OF_FLOW | ServoRestyleDamage::REFLOW);
        }

        if assign_inline_sizes.should_process(flow) {
            assign_inline_sizes.process(flow);
        }

        for kid in flow.mut_base().child_iter_mut() {
            doit(kid, assign_inline_sizes, assign_block_sizes, relayout_mode);
        }

        if assign_block_sizes.should_process(flow) {
            assign_block_sizes.process(flow);
        }
    }

    if opts::get().debug.bubble_inline_sizes_separately {
        let bubble_inline_sizes = BubbleISizes {
            layout_context: &layout_context,
        };
        bubble_inline_sizes.traverse(root);
    }

    let assign_inline_sizes = AssignISizes {
        layout_context: &layout_context,
    };
    let assign_block_sizes = AssignBSizes {
        layout_context: &layout_context,
    };

    doit(root, assign_inline_sizes, assign_block_sizes, relayout_mode);
}

pub fn build_display_list_for_subtree<'a>(
    flow_root: &mut dyn Flow,
    layout_context: &'a LayoutContext,
    background_color: ColorF,
    client_size: Size2D<Au>,
) -> DisplayListBuildState<'a> {
    let mut state = StackingContextCollectionState::new(layout_context.id);
    flow_root.collect_stacking_contexts(&mut state);

    let mut state = DisplayListBuildState::new(layout_context, state);

    // Create a base rectangle for the page background based on the root
    // background color.
    let bounds = Rect::new(Point2D::new(Au::new(0), Au::new(0)), client_size);
    let base = state.create_base_display_item(
        bounds,
        flow_root.as_block().fragment.node,
        None,
        DisplayListSection::BackgroundAndBorders,
    );
    state.add_display_item(DisplayItem::Rectangle(CommonDisplayItem::new(
        base,
        RectangleDisplayItem {
            color: PropertyBinding::Value(background_color),
            common: items::empty_common_item_properties(),
            bounds: bounds.to_layout(),
        },
    )));

    let mut build_display_list = BuildDisplayList { state: state };
    build_display_list.traverse(flow_root);
    build_display_list.state
}

pub fn iterate_through_flow_tree_fragment_border_boxes(
    root: &mut dyn Flow,
    iterator: &mut dyn FragmentBorderBoxIterator,
) {
    fn doit(
        flow: &mut dyn Flow,
        level: i32,
        iterator: &mut dyn FragmentBorderBoxIterator,
        stacking_context_position: &Point2D<Au>,
    ) {
        flow.iterate_through_fragment_border_boxes(iterator, level, stacking_context_position);

        for kid in flow.mut_base().child_iter_mut() {
            let mut stacking_context_position = *stacking_context_position;
            if kid.is_block_flow() && kid.as_block().fragment.establishes_stacking_context() {
                stacking_context_position =
                    Point2D::new(kid.as_block().fragment.margin.inline_start, Au(0)) +
                        kid.base().stacking_relative_position +
                        stacking_context_position.to_vector();
                let relative_position = kid
                    .as_block()
                    .stacking_relative_border_box(CoordinateSystem::Own);
                if let Some(matrix) = kid.as_block().fragment.transform_matrix(&relative_position) {
                    let transform_matrix = matrix.transform_point2d(LayoutPoint::zero()).unwrap();
                    stacking_context_position = stacking_context_position +
                        Vector2D::new(
                            Au::from_f32_px(transform_matrix.x),
                            Au::from_f32_px(transform_matrix.y),
                        )
                }
            }
            doit(kid, level + 1, iterator, &stacking_context_position);
        }
    }

    doit(root, 0, iterator, &Point2D::zero());
}

pub fn store_overflow(layout_context: &LayoutContext, flow: &mut dyn Flow) {
    if !flow
        .base()
        .restyle_damage
        .contains(ServoRestyleDamage::STORE_OVERFLOW)
    {
        return;
    }

    for kid in flow.mut_base().child_iter_mut() {
        store_overflow(layout_context, kid);
    }

    flow.store_overflow(layout_context);

    flow.mut_base()
        .restyle_damage
        .remove(ServoRestyleDamage::STORE_OVERFLOW);
}

/// Guesses how much inline size will be taken up by floats on the left and right sides of the
/// given flow. This is needed to speculatively calculate the inline sizes of block formatting
/// contexts. The speculation typically succeeds, but if it doesn't we have to lay it out again.
pub fn guess_float_placement(flow: &mut dyn Flow) {
    if !flow
        .base()
        .restyle_damage
        .intersects(ServoRestyleDamage::REFLOW)
    {
        return;
    }

    let mut floats_in = SpeculatedFloatPlacement::compute_floats_in_for_first_child(flow);
    for kid in flow.mut_base().child_iter_mut() {
        if kid
            .base()
            .flags
            .contains(FlowFlags::IS_ABSOLUTELY_POSITIONED)
        {
            // Do not propagate floats in or out, but do propagate between kids.
            guess_float_placement(kid);
        } else {
            floats_in.compute_floats_in(kid);
            kid.mut_base().speculated_float_placement_in = floats_in;
            guess_float_placement(kid);
            floats_in = kid.base().speculated_float_placement_out;
        }
    }
    floats_in.compute_floats_out(flow);
    flow.mut_base().speculated_float_placement_out = floats_in
}
