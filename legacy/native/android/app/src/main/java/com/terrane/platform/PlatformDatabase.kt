package com.terrane.platform

import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import android.util.Log

class PlatformDatabase(private val context: Context) : SQLiteOpenHelper(context, "platform.sqlite", null, 1) {
    override fun onConfigure(db: SQLiteDatabase) {
        db.setForeignKeyConstraintsEnabled(true)
    }

    override fun onCreate(db: SQLiteDatabase) {
        applyCheckedInMigrations(db)
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) {
        applyCheckedInMigrations(db)
    }

    override fun onOpen(db: SQLiteDatabase) {
        super.onOpen(db)
        db.execSQL("PRAGMA foreign_keys = ON")
        applyCheckedInMigrations(db)
        runIntegrityCheck(db)
    }

    private fun applyCheckedInMigrations(db: SQLiteDatabase) {
        val migrations = context.assets.list("db/sqlite")
            ?.filter { it.endsWith(".sql") }
            ?.sorted()
            .orEmpty()
        if (migrations.isEmpty()) {
            Log.e("TerranePlatformDatabase", "db/sqlite migrations are missing from Android assets")
            return
        }

        for (migration in migrations) {
            context.assets.open("db/sqlite/$migration").bufferedReader().use { reader ->
                executeScript(db, reader.readText())
            }
        }
    }

    private fun executeScript(db: SQLiteDatabase, script: String) {
        val withoutComments = script
            .lineSequence()
            .filterNot { it.trimStart().startsWith("--") }
            .joinToString("\n")
        for (statement in withoutComments.split(";").map { it.trim() }.filter { it.isNotEmpty() }) {
            db.execSQL(statement)
        }
    }

    private fun runIntegrityCheck(db: SQLiteDatabase) {
        db.rawQuery("PRAGMA integrity_check", emptyArray()).use { cursor ->
            if (cursor.moveToFirst() && cursor.getString(0) != "ok") {
                Log.e("TerranePlatformDatabase", "PRAGMA integrity_check failed: ${cursor.getString(0)}")
            }
        }
    }
}
