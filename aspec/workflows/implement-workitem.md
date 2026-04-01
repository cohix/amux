# Implement Feature Workflow

## Step: plan
Prompt: Read work item {{work_item_number}} and produce a detailed implementation plan. Do not write any code yet. Write the plan to `./aspec/work-items/plans/{{work_item_number}}-{name}-plan.md`.

## Step: implement
Depends-on: plan
Prompt: Implement work item {{work_item_number}} according to the plan produced in the previous step. Iterate until the work item is comprehensively implemented, the build succeeds, and all existing tests pass. DO NOT write any new tests yet, just fix any you break. New tests will be implemented in the next step.

Follow the plan you wrote and compare against the work item implementation spec:

{{work_item_section:[Implementation Details]}}

## Step: docs
Depends-on: implement
Prompt: Write comprehensive documentation for work item {{work_item_number}}, following the plan that was previously written and following guidelines from the project aspec.

## Step: tests
Depends-on: implement
Prompt: Implement tests for work item {{work_item_number}} as described in the project aspec and the work item test considerations below:

{{work_item_section:[Test Considerations]}}

## Step: review
Depends-on: docs,tests
Prompt: Review the changes made for work item {{work_item_number}} in the previous steps for correctness, completeness, security, and style. Suggest improvements if needed. Ensure all edge cases are considered:

{{work_item_section:[Edge Case Considerations]}}

Ensure tests were implemented as described below:

{{work_item_section:[Test Considerations]}}

When complete, provide a short manual test plan and give me a chance to manually test and make any tweaks needed with freeform chat.
