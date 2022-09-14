using System;
using System.Collections.Generic;
using System.Text;

using System.Configuration;
using System.Data.Common;
using System.Resources;

namespace s3b
{
    public abstract class PersistBase
    {
        public delegate void SelectCallback(DbDataReader rdr);

        public abstract  void execCmd(Model parameters, string sqlTemplate);
        public abstract  void execCmd(string sql);

        public abstract void select(SelectCallback selectCallback, string sql);

        protected abstract string insertSql(Model m);
        protected abstract string updateSql(Model m);

        protected abstract string toSql(object o);
        protected abstract long identity();

        protected Dictionary<string, string> sqlTemplates;


        public void get(Model t)
        {
            string sql = getSql(t);

            SelectCallback sc = (rdr) =>
            {
                autoAssign(rdr, t);
            };

            select(sc, sql);
        }


        public void autoAssign(DbDataReader rdr, Model t)
        {
            for (int i = 0; i < rdr.FieldCount; i++)
            {
                string k = rdr.GetName(i);
                if (t.ContainsKey(k)) { t[k] = rdr.GetValue(i); }
                else { t.Add(k, rdr.GetValue(i)); }
            }
        }

        protected string substituteTemplate(Model parameters, string sqlTemplate)
        {
            Template t = new Template(sqlTemplate);
            return t.eval(parameters);
        }

        protected string getSql(Model t)
        {
            string sqlTemplate = "select * from $(table) where id=$(id)";

            s3b.Model p = new Model();

            p["id"] = t["id"];
            p["table"] = t.tableName;

            return substituteTemplate(p, sqlTemplate);

        }

        
        public virtual void select(SelectCallback selectCallback, string sqlTemplate, Model filter)
        {
            string sql = substituteTemplate(filter, sqlTemplate);

            Logger.debug("sql = " + sql);

            select(selectCallback, sql);
        }

        public  virtual void insert(Model model)
        {
            model["id"] = identity();
            string sql = insertSql(model);
            execCmd(sql);
        }

        public virtual void update(Model model)
        {
            string sql = updateSql(model);
            execCmd(sql);
        }

        public virtual void put(Model model, string rwkCol)
        {
            string sql = "select * from $(tableName) where $(rwkCol) = $(rwkValue)";
            bool found = false;

            Model filter = new Model();
            filter["tableName"] = model.tableName;
            filter["rwkCol"] = rwkCol;
            filter["rwkValue"] = toSql(model[rwkCol].ToString());

            SelectCallback scb = (rdr) =>
            {
                found = true;
                model["id"] = rdr["id"];
            };

            select(scb, sql, filter);

            if (found)
            {
                update(model);
            }
            else
            {
                insert(model);
            }
        }

        public virtual void query(SelectCallback scb, string queryName )
        {
            string sql = getTemplate(queryName);

            select(scb, sql);
        }

        private  string getTemplate(string queryName)
        {
            string sql = queryName;

            if (sqlTemplates.ContainsKey(queryName))
            {
                sql = sqlTemplates[queryName];
            }

            return sql;
        }

        public virtual void query(SelectCallback scb, string queryName, Model filter)
        {
            string sqlTemplate = getTemplate(queryName);

            select(scb, sqlTemplate, filter);
        }
    }
}