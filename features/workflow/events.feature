Feature: Workflow events
  The workflow reports its progress as events. The receiver of the events
  can cancel the run.

  Scenario: The event receiver cancels the run before the split
    Given a local album "album.flac" with 1 chapter
    And the run is cancelled when the plan is ready
    When the workflow runs for the local file
    Then the workflow is cancelled
    And the first event is the input classification
    And an event reports a plan with 1 album and 0 singles
    And nothing was split

  Scenario: Events report the track progress and the completion
    Given a local album "album.flac" with 1 chapter
    When the workflow runs for the local file
    Then an event reports split progress 1 of 1 at chapter 1
    And the last event is the completion
