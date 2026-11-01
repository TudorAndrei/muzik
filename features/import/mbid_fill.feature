Feature: Filling MusicBrainz ids
  muzik finds the MusicBrainz release of a library album that has no
  MusicBrainz id. It writes only the ids and keeps all other tags and fields.

  Scenario: A strong match writes only MusicBrainz ids to the files and the library
    Given a new beets library with the album "Quiet Hours" by "Mara Vale" and these songs:
      | track | title         |
      | 1     | Morning Light |
      | 2     | Evening Rain  |
    And MusicBrainz has the release "Quiet Hours" by "Mara Vale" from 2020 with the id "release-1":
      | track | title         | artist    | recording | release track | length |
      | 1     | Morning Light | Mara Vale | rec-1     | rt-1          | 200    |
      | 2     | Evening Rain  | Mara Vale | rec-2     | rt-2          | 200    |
    And the release is in the release group "group-1" on "Digital Media"
    And MusicBrainz credits the artist "artist-1" for the album and for the release track "rt-1"
    Then the library has 1 album without a MusicBrainz id
    When muzik looks for the release of that album
    Then the match is strong
    When muzik writes the MusicBrainz ids of the best match
    Then the library has 0 albums without a MusicBrainz id
    And the library album has these fields:
      | mb_albumid        | release-1   |
      | mb_releasegroupid | group-1     |
      | mb_albumartistid  | artist-1    |
      | album             | Quiet Hours |
    And library song 1 has these fields:
      | mb_trackid        | rec-1    |
      | mb_releasetrackid | rt-1     |
      | mb_artistid       | artist-1 |
    And library song 2 has these fields:
      | mb_trackid  | rec-2 |
      | mb_artistid | NULL  |
    And every library song keeps its file and title and has no year
    And every song file keeps the artist "Mara Vale", the album "Quiet Hours", and its title
    And every song file has the tag "mb_albumid" set to "release-1"
    And no song file has the tag "date"
    And no song file has the tag "track"
    And the file of library song 1 has the tag "mb_releasetrackid" set to "rt-1"
