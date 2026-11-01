Feature: Sync targets
  A sync target names a device folder and the format settings for it.
  muzik keeps the targets in the config file with the other settings.

  Background:
    Given a config file with the Spotify client id "kept"

  Scenario: A saved target loads with the same settings
    When muzik saves the "phone" target for the "opus" preset at 160 kbps without covers
    Then the "phone" target loads with the same settings
    And the Spotify client id is still "kept"

  Scenario: An unknown target name gives the names of the known targets
    Given muzik saved the "phone" target for the "opus" preset at 160 kbps without covers
    When muzik loads the "snowsky" target
    Then the load fails with a message that names "phone"

  Scenario: A bitrate that is too high is refused
    Given muzik saved the "phone" target for the "opus" preset at 160 kbps without covers
    When muzik saves the "phone" target for the "opus" preset at 999 kbps without covers
    Then the save fails
