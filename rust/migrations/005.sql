
    CREATE TABLE data_revision (
        id INTEGER PRIMARY KEY CHECK(id=1),
        revision INTEGER NOT NULL
    );
    INSERT INTO data_revision (id, revision) VALUES (1, 0);
    CREATE TRIGGER revision_games_insert AFTER INSERT ON games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_games_update AFTER UPDATE ON games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_games_delete AFTER DELETE ON games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_account_games_insert AFTER INSERT ON account_games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_account_games_update AFTER UPDATE ON account_games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_account_games_delete AFTER DELETE ON account_games BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_game_analysis_insert AFTER INSERT ON game_analysis BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_game_analysis_update AFTER UPDATE ON game_analysis BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_game_analysis_delete AFTER DELETE ON game_analysis BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_accounts_insert AFTER INSERT ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_accounts_update AFTER UPDATE ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_accounts_delete AFTER DELETE ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;